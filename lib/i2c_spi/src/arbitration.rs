use crate::{
    backend::{Backend, BackendStart},
    topology::{ControllerConfig, PeripheralConfig},
    types::*,
};
use alloc::{collections::VecDeque, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientEndpoint {
    pub channel: u64,
    pub peripheral_node_id: u64,
    pub allowed_methods: Vec<u64>,
    pub lock: Option<LockLease>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockLease {
    pub owner_channel: u64,
    pub expires_at_ms: u64,
    pub expired: bool,
}

impl ClientEndpoint {
    pub fn new(channel: u64, peripheral_node_id: u64, allowed_methods: Vec<u64>) -> Self {
        Self {
            channel,
            peripheral_node_id,
            allowed_methods,
            lock: None,
        }
    }

    pub fn locked(
        channel: u64,
        peripheral_node_id: u64,
        allowed_methods: Vec<u64>,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            channel,
            peripheral_node_id,
            allowed_methods,
            lock: Some(LockLease {
                owner_channel: channel,
                expires_at_ms,
                expired: false,
            }),
        }
    }

    pub fn allows(&self, ordinal: u64) -> bool {
        self.allowed_methods.is_empty()
            || self
                .allowed_methods
                .iter()
                .any(|allowed| *allowed == ordinal)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedTransfer {
    pub request_id: u64,
    pub client_channel: u64,
    pub peripheral_node_id: u64,
    pub request: Request,
    pub deadline_ms: u64,
    pub lock_owner: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveTransfer {
    pub queued: QueuedTransfer,
    pub backend_token: u64,
    pub ready_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Completion {
    pub request_id: u64,
    pub client_channel: u64,
    pub outcome: TransferOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitResult {
    Queued(u64),
    Rejected(Status),
}

#[derive(Clone, Debug)]
pub struct BusController {
    pub config: ControllerConfig,
    pub clients: Vec<ClientEndpoint>,
    pub queue: VecDeque<QueuedTransfer>,
    pub active: Option<ActiveTransfer>,
    pub completions: VecDeque<Completion>,
    pub current_spi_config: Option<(u64, SpiMode, u32)>,
    next_request_id: u64,
    draining: bool,
}

impl BusController {
    pub fn new(config: ControllerConfig) -> Self {
        Self {
            config,
            clients: Vec::new(),
            queue: VecDeque::new(),
            active: None,
            completions: VecDeque::new(),
            current_spi_config: None,
            next_request_id: 1,
            draining: false,
        }
    }

    pub fn endpoint_count(&self) -> usize {
        self.clients.len()
    }

    pub fn add_endpoint(
        &mut self,
        channel: u64,
        peripheral_node_id: u64,
        allowed_methods: Vec<u64>,
    ) -> Result<(), Status> {
        self.peripheral(peripheral_node_id)?;
        self.clients.push(ClientEndpoint::new(
            channel,
            peripheral_node_id,
            allowed_methods,
        ));
        Ok(())
    }

    pub fn lock_bus(
        &mut self,
        client_channel: u64,
        locked_channel: u64,
        peripheral_node_id: u64,
        allowed_methods: Vec<u64>,
        lease_ms: u64,
        now_ms: u64,
    ) -> Result<u64, Status> {
        self.peripheral(peripheral_node_id)?;
        if self.active_lock_owner(now_ms).is_some() {
            return Err(Status::AlreadyExists);
        }
        let lease_ms = lease_ms.min(MAX_LOCK_LEASE_MS);
        if lease_ms == 0 {
            return Err(Status::InvalidArgs);
        }
        let expires_at = now_ms.saturating_add(lease_ms);
        self.clients.push(ClientEndpoint::locked(
            locked_channel,
            peripheral_node_id,
            allowed_methods,
            expires_at,
        ));
        let _ = client_channel;
        Ok(expires_at)
    }

    pub fn release_channel(&mut self, channel: u64) {
        self.clients.retain(|client| client.channel != channel);
        self.queue.retain(|queued| queued.client_channel != channel);
    }

    pub fn submit(
        &mut self,
        client_channel: u64,
        peripheral_node_id: u64,
        request: Request,
        requested_deadline_ms: u64,
        now_ms: u64,
    ) -> SubmitResult {
        if self.draining {
            return SubmitResult::Rejected(Status::BadState);
        }
        let client = match self
            .clients
            .iter()
            .find(|client| client.channel == client_channel)
        {
            Some(client) => client.clone(),
            None => return SubmitResult::Rejected(Status::InvalidHandle),
        };
        if client.peripheral_node_id != peripheral_node_id {
            return SubmitResult::Rejected(Status::AccessDenied);
        }
        self.expire_locks(now_ms);
        let lock_owner = match client.lock {
            Some(lease) if lease.expired || now_ms >= lease.expires_at_ms => {
                return SubmitResult::Rejected(Status::LockExpired);
            }
            Some(lease) => Some(lease.owner_channel),
            None => None,
        };
        if self.queue.len() >= MAX_PENDING_BUNDLES_PER_CONTROLLER {
            return SubmitResult::Rejected(Status::QueueFull);
        }
        if let Err(status) = self.validate_request(peripheral_node_id, &request) {
            return SubmitResult::Rejected(status);
        }
        let budget = if requested_deadline_ms == 0 {
            MAX_REQUEST_DEADLINE_MS
        } else {
            requested_deadline_ms.min(MAX_REQUEST_DEADLINE_MS)
        };
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.queue.push_back(QueuedTransfer {
            request_id,
            client_channel,
            peripheral_node_id,
            request,
            deadline_ms: now_ms.saturating_add(budget),
            lock_owner,
        });
        SubmitResult::Queued(request_id)
    }

    pub fn poll<B: Backend>(&mut self, backend: &mut B, now_ms: u64) -> bool {
        self.expire_locks(now_ms);
        let mut changed = false;
        if self
            .active
            .as_ref()
            .is_some_and(|active| now_ms >= active.queued.deadline_ms)
        {
            let active = self.active.take().unwrap();
            let outcome = backend.abort(active.backend_token);
            self.completions.push_back(Completion {
                request_id: active.queued.request_id,
                client_channel: active.queued.client_channel,
                outcome,
            });
            changed = true;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| now_ms >= active.ready_at_ms)
        {
            let active = self.active.take().unwrap();
            let outcome = backend.complete(active.backend_token);
            if let Request::Spi(bundle) = &active.queued.request {
                if outcome.status == Status::Ok {
                    self.current_spi_config = Some((
                        active.queued.peripheral_node_id,
                        bundle.mode,
                        bundle.speed_hz,
                    ));
                }
            }
            self.completions.push_back(Completion {
                request_id: active.queued.request_id,
                client_channel: active.queued.client_channel,
                outcome,
            });
            changed = true;
        }
        if self.active.is_none() && !self.draining {
            changed |= self.start_next(backend, now_ms);
        }
        changed
    }

    pub fn next_completion(&mut self) -> Option<Completion> {
        self.completions.pop_front()
    }

    pub fn begin_drain(&mut self) {
        self.draining = true;
    }

    pub fn quiescence_ready<B: Backend>(&mut self, backend: &mut B, now_ms: u64) -> bool {
        self.begin_drain();
        if let Some(active) = self.active.as_ref() {
            if now_ms >= active.ready_at_ms || now_ms >= active.queued.deadline_ms {
                self.poll(backend, now_ms);
            }
        }
        self.active.is_none()
    }

    pub fn validate_request(
        &self,
        peripheral_node_id: u64,
        request: &Request,
    ) -> Result<(), Status> {
        let peripheral = self.peripheral(peripheral_node_id)?;
        match (request, &peripheral.config) {
            (Request::I2c(bundle), PeripheralConfig::I2c(_)) => {
                validate_i2c_bundle(bundle).map(|_| ())
            }
            (Request::Spi(bundle), PeripheralConfig::Spi(spi)) => {
                validate_spi_bundle(bundle)?;
                if !spi.supports(bundle.mode, bundle.speed_hz) {
                    return Err(Status::InvalidArgs);
                }
                Ok(())
            }
            _ => Err(Status::AccessDenied),
        }
    }

    fn start_next<B: Backend>(&mut self, backend: &mut B, now_ms: u64) -> bool {
        let Some(front) = self.queue.front() else {
            return false;
        };
        if now_ms < front.deadline_ms && !self.can_start(front, now_ms) {
            return false;
        }
        let queued = self.queue.pop_front().unwrap();
        if now_ms >= queued.deadline_ms {
            self.completions.push_back(Completion {
                request_id: queued.request_id,
                client_channel: queued.client_channel,
                outcome: TransferOutcome {
                    status: Status::TimedOut,
                    reads: Vec::new(),
                    operations_completed: 0,
                },
            });
            return true;
        }
        match backend.start(
            self.config.node_id,
            queued.peripheral_node_id,
            &queued.request,
            now_ms,
        ) {
            Ok(BackendStart { token, ready_at_ms }) => {
                self.active = Some(ActiveTransfer {
                    queued,
                    backend_token: token,
                    ready_at_ms,
                });
            }
            Err(status) => {
                self.completions.push_back(Completion {
                    request_id: queued.request_id,
                    client_channel: queued.client_channel,
                    outcome: TransferOutcome {
                        status,
                        reads: Vec::new(),
                        operations_completed: 0,
                    },
                });
            }
        }
        true
    }

    fn can_start(&self, queued: &QueuedTransfer, now_ms: u64) -> bool {
        match self.active_lock_owner(now_ms) {
            Some(owner) => queued.lock_owner == Some(owner),
            None => true,
        }
    }

    fn active_lock_owner(&self, now_ms: u64) -> Option<u64> {
        self.clients.iter().find_map(|client| {
            let lease = client.lock.as_ref()?;
            (!lease.expired && now_ms < lease.expires_at_ms).then_some(lease.owner_channel)
        })
    }

    fn expire_locks(&mut self, now_ms: u64) {
        for client in &mut self.clients {
            if let Some(lease) = &mut client.lock {
                if now_ms >= lease.expires_at_ms {
                    lease.expired = true;
                }
            }
        }
    }

    fn peripheral(&self, node_id: u64) -> Result<&crate::topology::Peripheral, Status> {
        self.config
            .peripherals
            .iter()
            .find(|peripheral| peripheral.node_id == node_id)
            .ok_or(Status::NotFound)
    }
}
