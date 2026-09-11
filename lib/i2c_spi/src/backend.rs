use crate::{
    migration::{decode_request, encode_request},
    topology::Topology,
    types::*,
};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InjectedFailure {
    pub peripheral_node_id: u64,
    pub status: Status,
    pub remaining: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendStart {
    pub token: u64,
    pub ready_at_ms: u64,
}

pub trait Backend {
    fn start(
        &mut self,
        controller: u64,
        peripheral: u64,
        request: &Request,
        now_ms: u64,
    ) -> Result<BackendStart, Status>;
    fn complete(&mut self, token: u64) -> TransferOutcome;
    fn abort(&mut self, token: u64) -> TransferOutcome;
}

#[derive(Clone, Debug)]
struct ActiveTransfer {
    peripheral: u64,
    request: Request,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusEvent {
    I2cStart(u64),
    I2cRepeatedStart(u64),
    I2cStop(u64),
    SpiCsAssert(u64),
    SpiCsDeassert(u64),
}

#[derive(Clone, Debug, Default)]
pub struct DeterministicBackend {
    next_token: u64,
    active: BTreeMap<u64, ActiveTransfer>,
    registers: BTreeMap<u64, Vec<u8>>,
    spi_last_config: BTreeMap<u64, (SpiMode, u32)>,
    controller_delay_ms: BTreeMap<u64, u32>,
    failures: Vec<InjectedFailure>,
    events: Vec<BusEvent>,
    pub i2c_operations: u64,
    pub spi_operations: u64,
}

impl DeterministicBackend {
    pub fn new(topology: &Topology) -> Self {
        let mut backend = Self {
            next_token: 1,
            active: BTreeMap::new(),
            registers: BTreeMap::new(),
            spi_last_config: BTreeMap::new(),
            controller_delay_ms: BTreeMap::new(),
            failures: Vec::new(),
            events: Vec::new(),
            i2c_operations: 0,
            spi_operations: 0,
        };
        for (_, peripheral) in topology.peripherals_for_bus(BusKind::I2c) {
            backend.registers.insert(peripheral.node_id, Vec::new());
        }
        backend
    }

    pub fn inject_failure(&mut self, peripheral_node_id: u64, status: Status, count: u32) {
        self.failures.push(InjectedFailure {
            peripheral_node_id,
            status,
            remaining: count,
        });
    }

    pub fn set_delay(&mut self, controller_node_id: u64, delay_ms: u32) {
        self.controller_delay_ms.insert(
            controller_node_id,
            delay_ms.min(MAX_REQUEST_DEADLINE_MS as u32),
        );
    }

    pub fn register_bytes(&self, peripheral_node_id: u64) -> Option<&[u8]> {
        self.registers
            .get(&peripheral_node_id)
            .map(|bytes| bytes.as_slice())
    }

    pub fn spi_config(&self, peripheral_node_id: u64) -> Option<(SpiMode, u32)> {
        self.spi_last_config.get(&peripheral_node_id).copied()
    }

    pub fn events(&self) -> &[BusEvent] {
        &self.events
    }

    pub fn encode(&self, w: &mut bexos_migration::codec::Encoder) {
        w.word(1);
        w.word(self.next_token);
        w.word(self.active.len() as u64);
        for (token, active) in &self.active {
            w.word(*token);
            w.word(active.peripheral);
            encode_request(w, &active.request);
        }
        w.word(self.registers.len() as u64);
        for (node, bytes) in &self.registers {
            w.word(*node);
            w.bytes(bytes);
        }
        w.word(self.spi_last_config.len() as u64);
        for (node, (mode, speed)) in &self.spi_last_config {
            w.word(*node);
            w.word(*mode as u64);
            w.word(*speed as u64);
        }
        w.word(self.controller_delay_ms.len() as u64);
        for (node, delay) in &self.controller_delay_ms {
            w.word(*node);
            w.word(*delay as u64);
        }
        w.word(self.failures.len() as u64);
        for failure in &self.failures {
            w.word(failure.peripheral_node_id);
            w.word(status_to_word(failure.status));
            w.word(failure.remaining as u64);
        }
        w.word(self.events.len() as u64);
        for event in &self.events {
            match *event {
                BusEvent::I2cStart(node) => {
                    w.word(1);
                    w.word(node);
                }
                BusEvent::I2cRepeatedStart(node) => {
                    w.word(2);
                    w.word(node);
                }
                BusEvent::I2cStop(node) => {
                    w.word(3);
                    w.word(node);
                }
                BusEvent::SpiCsAssert(node) => {
                    w.word(4);
                    w.word(node);
                }
                BusEvent::SpiCsDeassert(node) => {
                    w.word(5);
                    w.word(node);
                }
            }
        }
        w.word(self.i2c_operations);
        w.word(self.spi_operations);
    }

    pub fn decode(
        r: &mut bexos_migration::codec::Decoder<'_>,
    ) -> Result<Self, bexos_migration::Error> {
        if r.word()? != 1 {
            return Err(bexos_migration::Error::UnsupportedVersion);
        }
        let mut backend = Self {
            next_token: r.word()?,
            ..Self::default()
        };
        for _ in 0..r.count(MAX_PENDING_BUNDLES_PER_CONTROLLER)? {
            let token = r.word()?;
            let peripheral = r.word()?;
            let request = decode_request(r)?;
            backend.active.insert(
                token,
                ActiveTransfer {
                    peripheral,
                    request,
                },
            );
        }
        for _ in 0..r.count(512)? {
            let node = r.word()?;
            let bytes = r.bytes(MAX_TRANSFER_BYTES_PER_BUNDLE)?.to_vec();
            backend.registers.insert(node, bytes);
        }
        for _ in 0..r.count(512)? {
            let node = r.word()?;
            let mode = match r.word()? {
                0 => SpiMode::Mode0,
                1 => SpiMode::Mode1,
                2 => SpiMode::Mode2,
                3 => SpiMode::Mode3,
                _ => return Err(bexos_migration::Error::InvalidData),
            };
            let speed =
                u32::try_from(r.word()?).map_err(|_| bexos_migration::Error::InvalidData)?;
            backend.spi_last_config.insert(node, (mode, speed));
        }
        for _ in 0..r.count(512)? {
            let node = r.word()?;
            let delay =
                u32::try_from(r.word()?).map_err(|_| bexos_migration::Error::InvalidData)?;
            backend.controller_delay_ms.insert(node, delay);
        }
        for _ in 0..r.count(512)? {
            backend.failures.push(InjectedFailure {
                peripheral_node_id: r.word()?,
                status: word_to_status(r.word()?).ok_or(bexos_migration::Error::InvalidData)?,
                remaining: u32::try_from(r.word()?)
                    .map_err(|_| bexos_migration::Error::InvalidData)?,
            });
        }
        for _ in 0..r.count(4096)? {
            let kind = r.word()?;
            let node = r.word()?;
            backend.events.push(match kind {
                1 => BusEvent::I2cStart(node),
                2 => BusEvent::I2cRepeatedStart(node),
                3 => BusEvent::I2cStop(node),
                4 => BusEvent::SpiCsAssert(node),
                5 => BusEvent::SpiCsDeassert(node),
                _ => return Err(bexos_migration::Error::InvalidData),
            });
        }
        backend.i2c_operations = r.word()?;
        backend.spi_operations = r.word()?;
        Ok(backend)
    }

    fn delay_for(&self, controller: u64) -> u32 {
        self.controller_delay_ms
            .get(&controller)
            .copied()
            .or_else(|| self.controller_delay_ms.get(&0).copied())
            .unwrap_or(0)
    }

    fn take_failure(&mut self, peripheral: u64) -> Option<Status> {
        let index = self.failures.iter().position(|failure| {
            failure.peripheral_node_id == peripheral && failure.remaining > 0
        })?;
        let failure = &mut self.failures[index];
        failure.remaining -= 1;
        let status = failure.status;
        self.failures.retain(|failure| failure.remaining > 0);
        Some(status)
    }
}

impl Backend for DeterministicBackend {
    fn start(
        &mut self,
        controller: u64,
        peripheral: u64,
        request: &Request,
        now_ms: u64,
    ) -> Result<BackendStart, Status> {
        let token = self.next_token;
        self.next_token = self.next_token.checked_add(1).ok_or(Status::BadState)?;
        self.active.insert(
            token,
            ActiveTransfer {
                peripheral,
                request: request.clone(),
            },
        );
        Ok(BackendStart {
            token,
            ready_at_ms: now_ms.saturating_add(self.delay_for(controller) as u64),
        })
    }

    fn complete(&mut self, token: u64) -> TransferOutcome {
        let Some(active) = self.active.remove(&token) else {
            return TransferOutcome {
                status: Status::BadState,
                reads: Vec::new(),
                operations_completed: 0,
            };
        };
        if let Some(status) = self.take_failure(active.peripheral) {
            return TransferOutcome {
                status,
                reads: Vec::new(),
                operations_completed: 0,
            };
        }
        match active.request {
            Request::I2c(bundle) => self.complete_i2c(active.peripheral, &bundle),
            Request::Spi(bundle) => self.complete_spi(active.peripheral, &bundle),
        }
    }

    fn abort(&mut self, token: u64) -> TransferOutcome {
        self.active.remove(&token);
        TransferOutcome {
            status: Status::TimedOut,
            reads: Vec::new(),
            operations_completed: 0,
        }
    }
}

impl DeterministicBackend {
    fn complete_i2c(&mut self, peripheral: u64, bundle: &I2cBundle) -> TransferOutcome {
        let mut reads = Vec::new();
        let register = self.registers.entry(peripheral).or_default();
        let mut cursor = 0usize;
        let mut completed = 0u32;
        self.events.push(BusEvent::I2cStart(peripheral));
        for op in &bundle.operations {
            if completed > 0 {
                self.events.push(BusEvent::I2cRepeatedStart(peripheral));
            }
            match op {
                I2cOp::Write(bytes) => {
                    *register = bytes.clone();
                    cursor = 0;
                }
                I2cOp::Read(len) => {
                    let mut out = Vec::with_capacity(*len);
                    for offset in 0..*len {
                        let source = if register.is_empty() {
                            ((peripheral as usize + offset) & 0xff) as u8
                        } else {
                            register[(cursor + offset) % register.len()]
                        };
                        out.push(source);
                    }
                    cursor = cursor.saturating_add(*len);
                    reads.push(ReadChunk { bytes: out });
                }
            }
            completed = completed.saturating_add(1);
            self.i2c_operations = self.i2c_operations.saturating_add(1);
        }
        self.events.push(BusEvent::I2cStop(peripheral));
        TransferOutcome::ok(reads, completed)
    }

    fn complete_spi(&mut self, peripheral: u64, bundle: &SpiBundle) -> TransferOutcome {
        self.spi_last_config
            .insert(peripheral, (bundle.mode, bundle.speed_hz));
        let mut reads = Vec::new();
        let mut completed = 0u32;
        self.events.push(BusEvent::SpiCsAssert(peripheral));
        for op in &bundle.operations {
            match op {
                SpiOp::Write(_) => {}
                SpiOp::Read(len) => {
                    let mut out = Vec::with_capacity(*len);
                    for index in 0..*len {
                        out.push(((peripheral as usize + index) ^ 0xa5) as u8);
                    }
                    reads.push(ReadChunk { bytes: out });
                }
                SpiOp::FullDuplex(bytes) => reads.push(ReadChunk {
                    bytes: bytes.iter().map(|byte| byte ^ 0xff).collect(),
                }),
            }
            completed = completed.saturating_add(1);
            self.spi_operations = self.spi_operations.saturating_add(1);
        }
        self.events.push(BusEvent::SpiCsDeassert(peripheral));
        TransferOutcome::ok(reads, completed)
    }
}

pub fn status_to_word(status: Status) -> u64 {
    match status {
        Status::Ok => 0,
        Status::InvalidHandle => 1,
        Status::AccessDenied => 2,
        Status::NoMemory => 3,
        Status::BufferTooSmall => 4,
        Status::PeerClosed => 5,
        Status::TimedOut => 6,
        Status::AlreadyExists => 7,
        Status::InvalidArgs => 8,
        Status::NotFound => 9,
        Status::BadState => 10,
        Status::Unsupported => 11,
        Status::Nack => 20,
        Status::ArbitrationLost => 21,
        Status::QueueFull => 22,
        Status::TooLarge => 23,
        Status::LockExpired => 24,
        Status::InjectedFailure => 25,
    }
}

pub fn word_to_status(word: u64) -> Option<Status> {
    Some(match word {
        0 => Status::Ok,
        1 => Status::InvalidHandle,
        2 => Status::AccessDenied,
        3 => Status::NoMemory,
        4 => Status::BufferTooSmall,
        5 => Status::PeerClosed,
        6 => Status::TimedOut,
        7 => Status::AlreadyExists,
        8 => Status::InvalidArgs,
        9 => Status::NotFound,
        10 => Status::BadState,
        11 => Status::Unsupported,
        20 => Status::Nack,
        21 => Status::ArbitrationLost,
        22 => Status::QueueFull,
        23 => Status::TooLarge,
        24 => Status::LockExpired,
        25 => Status::InjectedFailure,
        _ => return None,
    })
}
