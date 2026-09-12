use crate::{binding::Client, service::Service};
use alloc::vec::Vec;
use bexos_component_config::{schema::Error, transaction::Phase};
use bexos_userspace::{
    Channel, Startup,
    live_migration::{RecordChanges, Source},
    preferences as rpc,
};
use fidl::{FidlDecode, HandleRef, Status};
use preferences_fidl as fidl;

pub struct Runtime {
    pub incoming: Vec<Option<Vec<u8>>>,
    pub incoming_len: usize,
    pub control: Channel,
    pub migration: Option<Channel>,
    pub vfsd: Channel,
    pub usersd: Channel,
    pub watcher: Channel,
    pub clients: Vec<Client>,
    pub service: Service,
    pub timeout_ms: u64,
    pub generation: u64,
}
impl Runtime {
    pub fn empty() -> Self {
        Self {
            incoming: Vec::new(),
            incoming_len: 0,
            control: Channel(0),
            migration: None,
            vfsd: Channel(0),
            usersd: Channel(0),
            watcher: Channel(0),
            clients: Vec::new(),
            service: Service::default(),
            timeout_ms: 5000,
            generation: 0,
        }
    }
    pub fn check_user(&mut self, uid: u64) -> Result<(), Error> {
        if uid == 0 {
            return Ok(());
        }
        use user_manager_fidl::{
            FidlDecode, FidlEncode, UserManagerGetUserRequest, UserManagerGetUserResponse,
            UserStatus,
        };
        let mut bytes = [0; 16];
        let e = UserManagerGetUserRequest { uid }
            .encode(&mut bytes, &mut [])
            .map_err(|_| Error::Malformed)?;
        let m = bexos_userspace::Rpc(self.usersd)
            .call_raw(2, &bytes[..e.bytes], &[], true)
            .map_err(|_| Error::Storage)?;
        let r = UserManagerGetUserResponse::decode(&m.bytes, &[]).map_err(|_| Error::Malformed)?;
        if r.status != UserStatus::Ok || !r.user.unlocked || r.user.disabled {
            self.lock(uid);
            return Err(Error::AccessDenied);
        }
        self.service.locked.remove(&uid);
        Ok(())
    }
    pub fn lock(&mut self, uid: u64) {
        let affected = self.service.pending.as_ref().is_some_and(|p| {
            matches!(&p.mutation,crate::service::Mutation::User{uid:u,..}if *u==uid)
                || self
                    .service
                    .observers
                    .iter()
                    .any(|o| o.uid == uid && p.transaction.participants.contains(&o.channel))
        });
        if affected {
            self.abort(Status::AccessDenied);
        }
        let revoked = self
            .service
            .observers
            .iter()
            .filter(|o| o.uid == uid)
            .map(|o| o.channel)
            .collect::<Vec<_>>();
        if let Some(p) = &mut self.service.pending {
            for channel in &revoked {
                let _ = p.transaction.disconnect(*channel);
            }
            p.snapshots.retain(|s| !revoked.contains(&s.receiver));
            if matches!(p.transaction.phase, Phase::Committed | Phase::Complete)
                && p.reply_channel != 0
            {
                crate::wire::mutation_reply(
                    p.reply_channel,
                    p.reply_ordinal,
                    Status::CommittedPending,
                    p.response_generation,
                );
                p.reply_channel = 0;
            }
        }
        for o in self.service.observers.iter().filter(|o| o.uid == uid) {
            let _ = bexos_userspace::Memory::close(o.channel);
        }
        if let Some(p) = &mut self.service.pending {
            if let crate::service::Mutation::User { uid: u, values, .. } = &mut p.mutation {
                if *u == uid {
                    values.clear();
                }
            }
        }
        self.service.lock_user(uid);
        self.clients.retain(|c| {
            if !c.admin && c.uid == uid {
                let _ = bexos_userspace::Memory::close(c.channel);
                false
            } else {
                true
            }
        });
    }
    pub fn delete_user(&mut self, uid: u64) {
        self.lock(uid);
        // The encrypted filesystem no longer exists. An indeterminate decision
        // cannot be replayed into a newly created account that reuses this UID.
        if self.service.pending.as_ref().is_some_and(
            |p| matches!(&p.mutation,crate::service::Mutation::User{uid:u,..}if *u==uid),
        ) {
            if let Some(p) = self.service.pending.take() {
                crate::wire::mutation_reply(
                    p.reply_channel,
                    p.reply_ordinal,
                    Status::Storage,
                    p.response_generation,
                );
            }
        }
    }
    pub fn load(&mut self, key: &str, uid: u64) -> Result<(), Error> {
        self.check_user(uid)?;
        let package = self.service.package(key)?.clone();
        let siblings = self
            .service
            .packages
            .values()
            .filter(|p| {
                p.id == package.id && p.schema.fingerprint() == package.schema.fingerprint()
            })
            .map(|p| p.key.clone())
            .collect::<Vec<_>>();
        for sibling in &siblings {
            crate::storage::load_operator(self.vfsd, &mut self.service, sibling)?;
        }
        let persisted = crate::storage::load_user(self.vfsd, &mut self.service, key, uid)?;
        let mut candidate = self.service.clone();
        let mut changed = false;
        for sibling in &siblings {
            changed |= candidate.ensure_user(sibling, uid)?;
        }
        let prefs_key = Service::prefs_key(&package, uid);
        let has_user_values = candidate
            .preferences
            .get(&prefs_key)
            .is_some_and(|prefs| !prefs.values.is_empty());
        if changed && (persisted || has_user_values) {
            crate::storage::persist_user(self.vfsd, &candidate, key, uid)?;
        }
        self.service = candidate;
        Ok(())
    }
    pub fn abort(&mut self, status: Status) {
        let Some(mut p) = self.service.pending.take() else {
            return;
        };
        if p.transaction.abort().is_err() {
            self.service.pending = Some(p);
            return;
        }
        for snapshot in &p.snapshots {
            if let Ok(table) = bexos_component_config::ConfigTable::parse(&snapshot.bytes) {
                let _ = rpc::send(
                    Channel(snapshot.receiver),
                    4,
                    &fidl::ConfigObserverAbortRequest {
                        transaction_id: p.transaction.generation,
                        generation: table.generation(),
                    },
                );
            }
        }
        crate::wire::mutation_reply(
            p.reply_channel,
            p.reply_ordinal,
            status,
            p.response_generation.saturating_sub(1),
        );
    }
    pub fn begin_delivery(&mut self) -> Result<(), Error> {
        let p = self.service.pending.as_ref().ok_or(Error::Busy)?;
        for s in &p.snapshots {
            let table = bexos_component_config::ConfigTable::parse(&s.bytes)
                .map_err(|_| Error::Malformed)?;
            let raw = rpc::read_only_vmo(&s.bytes)?;
            let result = rpc::send(
                Channel(s.receiver),
                2,
                &fidl::ConfigObserverPrepareRequest {
                    transaction_id: p.transaction.generation,
                    generation: table.generation(),
                    config: HandleRef { raw },
                    config_len: s.bytes.len() as u64,
                },
            );
            if result.is_err() {
                let _ = bexos_userspace::Memory::close(raw);
                return result;
            }
        }
        Ok(())
    }
    fn poll_observers(&mut self) {
        let observers = self.service.observers.clone();
        for o in observers {
            let m = match Channel(o.channel).try_recv() {
                Ok(m) => m,
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    if let Some(p) = &mut self.service.pending {
                        if matches!(
                            p.transaction.phase,
                            Phase::Resolving | Phase::Committed | Phase::Complete
                        ) {
                            p.delivery_failed = true;
                            p.snapshots.retain(|s| s.receiver != o.channel);
                        }
                        if p.transaction.disconnect(o.channel).is_err() {
                            self.abort(Status::Rejected);
                        }
                    }
                    self.service.observers.retain(|x| x.channel != o.channel);
                    let _ = bexos_userspace::Memory::close(o.channel);
                    continue;
                }
                Err(_) => continue,
            };
            if !m.handles.is_empty() {
                for h in m.handles {
                    let _ = bexos_userspace::Memory::close(h);
                }
                if self
                    .service
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.transaction.participants.contains(&o.channel))
                {
                    self.abort(Status::Rejected);
                }
                continue;
            }
            if !o.registered {
                if m.bytes.get(..8) == Some(&1u64.to_le_bytes()[..])
                    && fidl::ConfigObserverRegisterRequest::decode(&m.bytes[8..], &[]).is_ok()
                {
                    let status = if self.service.pending.is_some() || self.service.frozen {
                        Status::Busy
                    } else {
                        if let Some(o) = self
                            .service
                            .observers
                            .iter_mut()
                            .find(|x| x.channel == o.channel)
                        {
                            o.registered = true;
                        }
                        Status::Ok
                    };
                    let snapshot = if status == Status::Ok {
                        self.service
                            .effective(&o.package, o.uid)
                            .ok()
                            .and_then(|(_, b)| {
                                rpc::read_only_vmo(&b).ok().map(|raw| (raw, b.len() as u64))
                            })
                    } else {
                        None
                    };
                    let handles = snapshot
                        .map(|(raw, _)| HandleRef { raw })
                        .into_iter()
                        .collect::<Vec<_>>();
                    let status = if status == Status::Ok && snapshot.is_none() {
                        Status::Storage
                    } else {
                        status
                    };
                    let _ = rpc::reply(
                        Channel(o.channel),
                        &fidl::ConfigObserverRegisterResponse {
                            status,
                            config: &handles,
                            config_len: snapshot.map_or(0, |(_, len)| len),
                        },
                    );
                }
                continue;
            }
            let Some(p) = &mut self.service.pending else {
                continue;
            };
            let expected = p
                .snapshots
                .iter()
                .find(|s| s.receiver == o.channel)
                .and_then(|s| bexos_component_config::ConfigTable::parse(&s.bytes).ok())
                .map(|t| t.generation());
            if !p.transaction.participants.contains(&o.channel) {
                continue;
            }
            match p.transaction.phase {
                Phase::Preparing => {
                    let response = fidl::ConfigObserverPrepareResponse::decode(&m.bytes, &[]);
                    if response
                        .as_ref()
                        .is_ok_and(|r| r.transaction_id != p.transaction.generation)
                    {
                        continue;
                    }
                    let accepted = response
                        .is_ok_and(|r| r.status == Status::Ok && Some(r.generation) == expected);
                    if p.transaction
                        .prepare_reply(o.channel, p.transaction.generation, accepted)
                        .is_err()
                    {
                        self.abort(Status::Rejected);
                    }
                }
                Phase::Committed => {
                    if fidl::ConfigObserverCommitResponse::decode(&m.bytes, &[]).is_ok_and(|r| {
                        r.transaction_id == p.transaction.generation
                            && r.status == Status::Ok
                            && Some(r.generation) == expected
                    }) {
                        let _ = p
                            .transaction
                            .commit_reply(o.channel, p.transaction.generation);
                    }
                }
                _ => {}
            }
        }
    }
    fn advance(&mut self) {
        let Some(p) = &mut self.service.pending else {
            return;
        };
        if p.transaction.expire(now_ms()).is_err() {
            self.abort(Status::Timeout);
            return;
        }
        if matches!(p.transaction.phase, Phase::Prepared | Phase::Resolving) {
            let resolving = p.transaction.phase == Phase::Resolving;
            if resolving
                && crate::storage::restore_candidate_user(self.vfsd, &mut self.service).is_err()
            {
                return;
            }
            let mut candidate = match if resolving {
                Ok(self.service.clone())
            } else {
                self.service.candidate()
            } {
                Ok(c) => c,
                Err(e) => {
                    self.abort(rpc::status(e));
                    return;
                }
            };
            let mutation = candidate.pending.as_ref().unwrap().mutation.clone();
            if let Err(e) = crate::storage::persist_mutation(self.vfsd, &candidate, &mutation) {
                if e == Error::CommitUncertain {
                    candidate.pending.as_mut().unwrap().transaction.phase = Phase::Resolving;
                    self.service = candidate;
                } else if self.service.pending.as_ref().unwrap().transaction.phase
                    != Phase::Resolving
                {
                    self.abort(rpc::status(e));
                }
                return;
            }
            {
                let p = candidate.pending.as_mut().unwrap();
                let _ = p.transaction.durable_commit();
            }
            crate::wire::broadcast_theme_changes(&mut candidate, &mutation);
            let p = candidate.pending.as_mut().unwrap();
            for snapshot in &p.snapshots {
                let generation = bexos_component_config::ConfigTable::parse(&snapshot.bytes)
                    .unwrap()
                    .generation();
                let _ = rpc::send(
                    Channel(snapshot.receiver),
                    3,
                    &fidl::ConfigObserverCommitRequest {
                        transaction_id: p.transaction.generation,
                        generation,
                    },
                );
            }
            self.service = candidate;
        }
        let Some(p) = &self.service.pending else {
            return;
        };
        if p.transaction.phase == Phase::Complete {
            let p = self.service.pending.take().unwrap();
            crate::wire::mutation_reply(
                p.reply_channel,
                p.reply_ordinal,
                if p.delivery_failed {
                    Status::CommittedPending
                } else {
                    Status::Ok
                },
                p.response_generation,
            );
        } else if p.transaction.phase == Phase::Committed
            && now_ms() >= p.transaction.deadline_ms.saturating_add(5000)
        {
            let p = self.service.pending.as_mut().unwrap();
            if p.reply_channel != 0 {
                crate::wire::mutation_reply(
                    p.reply_channel,
                    p.reply_ordinal,
                    Status::CommittedPending,
                    p.response_generation,
                );
                p.reply_channel = 0;
            }
            // Retry is idempotent. Retain committed state through heart transplant.
            for snapshot in &p.snapshots {
                if p.transaction.awaiting.contains(&snapshot.receiver) {
                    let generation = bexos_component_config::ConfigTable::parse(&snapshot.bytes)
                        .unwrap()
                        .generation();
                    let _ = rpc::send(
                        Channel(snapshot.receiver),
                        3,
                        &fidl::ConfigObserverCommitRequest {
                            transaction_id: p.transaction.generation,
                            generation,
                        },
                    );
                }
            }
            p.transaction.deadline_ms = now_ms();
        }
    }
}
pub fn now_ms() -> u64 {
    ((bexos_userspace::syscall::ticks() as u128 * 1000)
        / bexos_userspace::syscall::frequency() as u128) as u64
}
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("prefsd startup");
    let mut runtime = if startup.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, startup.migration_generation)
            .expect("prefsd transplant")
    } else {
        let mut r = Runtime::empty();
        r.control = control;
        r.migration = startup.migration;
        for g in &startup.service_grants {
            match g.protocol.as_str() {
                "VfsManager" => r.vfsd = Channel(g.endpoint),
                "UserManager" => r.usersd = Channel(g.endpoint),
                _ => {}
            }
        }
        if let Some(config) = startup.config {
            if let Ok(b) = rpc::read_vmo(config, startup.config_len) {
                if let Ok(t) = bexos_component_config::ConfigTable::parse(&b) {
                    r.timeout_ms = t
                        .get_u64("prepare_timeout_ms")
                        .unwrap_or(5000)
                        .clamp(1, 60000);
                }
            }
        }
        Startup::ready(control).expect("prefsd ready");
        r
    };
    let mut source = Source::new(runtime.migration);
    let mut changes = RecordChanges::default();
    loop {
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            runtime.abort(Status::Busy);
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(m) = runtime.control.try_recv() {
            crate::binding::accept(runtime.control, m, &mut runtime.clients);
        }
        crate::wire::poll(&mut runtime);
        runtime.poll_observers();
        runtime.advance();
        // User-manager events are authoritative, and requests additionally check
        // current unlock state before touching an encrypted store.
        crate::wire::poll_users(&mut runtime);
        bexos_userspace::yield_now();
    }
}
