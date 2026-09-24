use crate::{
    Error, Result,
    cas::Cas,
    credentials::{Secret, Vault},
    oci::Oci,
    resolution::{self, Prepared},
    secure_state::{RepositoryState, SecureStore, TrustyStore},
    transport::LiveTransport,
};
use bexos_pkg_client::{ArtifactQuery, BlobDigest};
use bexos_pkg_config::Config;
use bexos_userspace::{
    Channel, Memory, Startup, fs, live_migration::Source, service_binding::ServiceBinding, vfs,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

pub struct Client {
    pub channel: u64,
    pub package: String,
    pub protocol: String,
    pub methods: Vec<u64>,
}
pub struct Waiter {
    pub channel: u64,
    pub caller: String,
    pub expected: Option<BlobDigest>,
}
pub struct Job {
    pub query: ArtifactQuery,
    pub source_digest: Option<BlobDigest>,
    pub waiters: Vec<Waiter>,
    pub attempt: u8,
    pub future: Option<Pin<Box<dyn Future<Output = Result<Prepared>>>>>,
}
pub struct Blob {
    pub handle: u64,
    pub length: u64,
}
pub struct SecretHandle {
    pub handle: u64,
    pub length: u64,
    pub sealed: bool,
}
pub struct Runtime {
    pub adoption_inventory: Option<crate::migration::Inventory>,
    pub transfers: crate::transfers::Transfers,
    pub directory: Channel,
    pub control: Channel,
    pub migration: Option<Channel>,
    pub vfs: Channel,
    pub network: Channel,
    pub tls: Channel,
    pub tee: Channel,
    pub time: Channel,
    pub cache_file: Channel,
    pub state_file: Channel,
    pub config: Config,
    pub clients: Vec<Client>,
    pub jobs: Vec<Job>,
    pub cas: Rc<RefCell<Option<Cas>>>,
    pub secure: Rc<RefCell<Option<TrustyStore>>>,
    pub blobs: BTreeMap<[u8; 32], Blob>,
    pub vault: Vault,
    pub secrets: BTreeMap<String, SecretHandle>,
}
impl Runtime {
    pub fn empty() -> Self {
        Self {
            adoption_inventory: None,
            transfers: crate::transfers::Transfers::new(16),
            directory: Channel(0),
            control: Channel(0),
            migration: None,
            vfs: Channel(0),
            network: Channel(0),
            tls: Channel(0),
            tee: Channel(0),
            time: Channel(0),
            cache_file: Channel(0),
            state_file: Channel(0),
            config: Config::decode(include_bytes!(env!("PKGD_CONFIG")))
                .expect("pkgd compiled config"),
            clients: Vec::new(),
            jobs: Vec::new(),
            cas: Rc::new(RefCell::new(None)),
            secure: Rc::new(RefCell::new(None)),
            blobs: BTreeMap::new(),
            vault: Vault::default(),
            secrets: BTreeMap::new(),
        }
    }
    pub fn open_storage(&mut self) -> Result<()> {
        if self.cache_file.0 == 0 || self.state_file.0 == 0 {
            let directory = vfs::get_system_data_directory(self.vfs, "bexos.service.pkgd")
                .map_err(|_| Error::Io)?;
            let flags = fs_fidl::OpenFlags::RIGHT_READABLE.0
                | fs_fidl::OpenFlags::RIGHT_WRITABLE.0
                | fs_fidl::OpenFlags::CREATE.0;
            let cache = fs::open(directory, "cache.redb", flags);
            let state = fs::open(directory, "secure-state.redb", flags);
            let _ = Memory::close(directory.0);
            match (cache, state) {
                (Ok(cache), Ok(state)) => {
                    if self.cache_file.0 != 0 {
                        let _ = Memory::close(self.cache_file.0);
                    }
                    if self.state_file.0 != 0 {
                        let _ = Memory::close(self.state_file.0);
                    }
                    self.cache_file = cache;
                    self.state_file = state;
                }
                (cache, state) => {
                    for file in [cache, state].into_iter().flatten() {
                        let _ = Memory::close(file.0);
                    }
                    return Err(Error::Io);
                }
            }
        }
        if self.cas.borrow().is_none() {
            bexos_userspace::log("pkgd: opening durable cache\n");
            let db = bexos_redb::open_or_create_with_store(
                bexos_redb::bounded::BoundedStore::new(
                    bexos_redb::retained::RetainedStore::new(
                        bexos_redb::bexos_fs::FileBlockStore::new(self.cache_file),
                    ),
                    self.config
                        .max_cache_bytes
                        .saturating_mul(3)
                        .saturating_add(16 * 1024 * 1024),
                )
                .map_err(|_| Error::ResourceExhausted)?,
            )
            .map_err(|_| Error::Io)?;
            *self.cas.borrow_mut() = Some(Cas::open(db, self.config.max_cache_bytes)?);
            bexos_userspace::log("pkgd: durable cache ready\n");
        }
        if self.secure.borrow().is_none() && self.tee.0 != 0 {
            bexos_userspace::log("pkgd: opening secure state index\n");
            let db = bexos_redb::open_or_create_with_store(
                bexos_redb::bounded::BoundedStore::new(
                    bexos_redb::retained::RetainedStore::new(
                        bexos_redb::bexos_fs::FileBlockStore::new(self.state_file),
                    ),
                    256 * 1024 * 1024,
                )
                .map_err(|_| Error::ResourceExhausted)?,
            )
            .map_err(|_| Error::Io)?;
            *self.secure.borrow_mut() = Some(TrustyStore::new(self.tee, db)?);
            bexos_userspace::log("pkgd: secure state index ready\n");
        }
        Ok(())
    }
    pub fn pause(&mut self) {
        for job in &mut self.jobs {
            job.future = None;
        }
        self.cas.borrow_mut().take();
        self.secure.borrow_mut().take();
    }
    pub fn enqueue(&mut self, client: &Client, query: ArtifactQuery) -> Result<()> {
        self.enqueue_source(client, query, None)
    }
    pub fn enqueue_source(
        &mut self,
        client: &Client,
        mut query: ArtifactQuery,
        source_digest: Option<BlobDigest>,
    ) -> Result<()> {
        query.validate()?;
        let repo = self.config.repository(&query).ok_or(Error::AccessDenied)?;
        if !self.config.permits(&client.package, &repo.id(), query.kind) {
            return Err(Error::AccessDenied);
        }
        if self.jobs.iter().map(|job| job.waiters.len()).sum::<usize>() >= self.config.max_waiters {
            return Err(Error::ResourceExhausted);
        }
        let waiter = Waiter {
            channel: client.channel,
            caller: client.package.clone(),
            expected: query.expected_digest.take(),
        };
        if let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.query == query && job.source_digest == source_digest)
        {
            job.waiters.push(waiter);
            return Ok(());
        }
        if self.jobs.len() >= self.config.max_inflight {
            return Err(Error::ResourceExhausted);
        }
        self.jobs.push(Job {
            query,
            source_digest,
            waiters: vec![waiter],
            attempt: 0,
            future: None,
        });
        Ok(())
    }
    pub fn cached(&mut self, caller: &str, digest: &BlobDigest) -> Result<BlobDigest> {
        self.connect_services();
        self.open_storage()?;
        let mut secure_guard = self.secure.borrow_mut();
        let secure = secure_guard.as_mut().ok_or(Error::Unavailable)?;
        let resolved = resolution::resolve_cached(
            &self.config,
            caller,
            digest,
            secure,
            self.cas.borrow_mut().as_mut().ok_or(Error::Io)?,
        )?;
        drop(secure_guard);
        let canonical = self.publish_payload(resolved.bytes)?;
        Ok(BlobDigest {
            hash_type: pkg_fidl::HashType::Sha256,
            digest: canonical,
        })
    }
    pub fn publish_payload(
        &mut self,
        payload: std::rc::Rc<crate::payload::Payload>,
    ) -> Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(payload.as_ref()).into();
        if self.blobs.contains_key(&digest) {
            return Ok(digest);
        }
        self.reserve_blob(payload.len() as u64)?;
        let length = payload.len() as u64;
        self.blobs.insert(
            digest,
            Blob {
                handle: payload.duplicate_handle()?,
                length,
            },
        );
        Ok(digest)
    }
    fn reserve_blob(&mut self, length: u64) -> Result<()> {
        while self.blobs.len() >= 128
            || self
                .blobs
                .values()
                .map(|b| b.length)
                .sum::<u64>()
                .saturating_add(length)
                > self.config.max_payload_bytes
        {
            let key = self
                .blobs
                .keys()
                .next()
                .copied()
                .ok_or(Error::ResourceExhausted)?;
            if let Some(blob) = self.blobs.remove(&key) {
                let _ = Memory::close(blob.handle);
            }
        }
        Ok(())
    }
    pub fn publish(&mut self, bytes: &[u8]) -> Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        if self.blobs.contains_key(&digest) {
            return Ok(digest);
        }
        self.reserve_blob(bytes.len() as u64)?;
        let writable = Memory::from_bytes(bytes).map_err(|_| Error::ResourceExhausted)?;
        let rights = kernel_fidl::Rights::TRANSFER.0
            | kernel_fidl::Rights::READ.0
            | kernel_fidl::Rights::MAP.0
            | kernel_fidl::Rights::DUPLICATE.0;
        let canonical = Memory::duplicate(writable, rights);
        let _ = Memory::close(writable);
        self.blobs.insert(
            digest,
            Blob {
                handle: canonical.map_err(|_| Error::ResourceExhausted)?,
                length: bytes.len() as u64,
            },
        );
        Ok(digest)
    }
    pub fn pump(&mut self) {
        let mut jobs = core::mem::take(&mut self.jobs);
        let mut active = jobs.iter().filter(|job| job.future.is_some()).count();
        // Repository trust transitions use one serialized revision chain. Different
        // artifacts may share that chain; running their refreshes concurrently
        // would repeatedly invalidate each other's verified metadata commits.
        let mut active_repositories: BTreeSet<_> = jobs
            .iter()
            .filter(|job| job.future.is_some())
            .map(|job| format!("{}/{}", job.query.registry_host, job.query.repository))
            .collect();
        for mut job in jobs.drain(..) {
            let repository = format!("{}/{}", job.query.registry_host, job.query.repository);
            job.waiters.retain(|waiter| {
                self.clients
                    .iter()
                    .any(|client| client.channel == waiter.channel)
            });
            if job.waiters.is_empty() {
                if job.future.is_some() {
                    active = active.saturating_sub(1);
                    active_repositories.remove(&repository);
                }
                continue;
            }
            if job.future.is_none() {
                // Two payload reservations bound staging memory independently of queued callers.
                if active >= 2 || active_repositories.contains(&repository) {
                    self.jobs.push(job);
                    continue;
                }
                active += 1;
                match self.start(&job.query, job.source_digest) {
                    Ok(future) => {
                        job.future = Some(future);
                        active_repositories.insert(repository.clone());
                    }
                    Err(error) => {
                        active = active.saturating_sub(1);
                        crate::wire::finish(self, &job, Err(error));
                        continue;
                    }
                }
            }
            let waker = Waker::from(Arc::new(Noop));
            let mut context = Context::from_waker(&waker);
            let outcome = job.future.as_mut().unwrap().as_mut().poll(&mut context);
            let Poll::Ready(result) = outcome else {
                self.jobs.push(job);
                continue;
            };
            job.future = None;
            active = active.saturating_sub(1);
            active_repositories.remove(&repository);
            let result = result.and_then(|prepared| {
                let mut secure_guard = self.secure.borrow_mut();
                let secure = secure_guard.as_mut().ok_or(Error::Unavailable)?;
                let current = secure.load(&prepared.repository)?;
                if current.as_ref().map_or(0, |state| state.revision) != prepared.previous_revision
                {
                    return Err(Error::TimedOut);
                }
                secure.commit(
                    &prepared.repository,
                    prepared.previous_revision,
                    &prepared.state,
                )?;
                self.cas.borrow_mut().as_mut().ok_or(Error::Io)?.put(
                    &prepared.resolved.digest.digest,
                    &prepared.resolved.bytes,
                    &Default::default(),
                )?;
                drop(secure_guard);
                self.publish_payload(prepared.resolved.bytes)
                    .map(|digest| BlobDigest {
                        hash_type: pkg_fidl::HashType::Sha256,
                        digest,
                    })
            });
            if matches!(result, Err(Error::TimedOut)) && job.attempt < 3 {
                bexos_userspace::log(&format!(
                    "pkgd: retrying {:?} resolution after timeout attempt={}\n",
                    job.query.kind,
                    job.attempt + 1,
                ));
                job.attempt += 1;
                self.jobs.push(job);
            } else {
                bexos_userspace::log(&format!(
                    "pkgd: {:?} resolution finished status={:?}\n",
                    job.query.kind,
                    result.as_ref().map(|_| ()),
                ));
                crate::wire::finish(self, &job, result);
            }
        }
    }
    fn start(
        &mut self,
        query: &ArtifactQuery,
        source_digest: Option<BlobDigest>,
    ) -> Result<Pin<Box<dyn Future<Output = Result<Prepared>>>>> {
        self.connect_services();
        self.open_storage()?;
        if self.directory.0 == 0 {
            bexos_userspace::log("pkgd: resolver service directory unavailable\n");
            return Err(Error::Unavailable);
        }
        let now = self.valid_time()?;
        let repo = self
            .config
            .repository(query)
            .ok_or(Error::AccessDenied)?
            .clone();
        crate::wire::load_sealed(self, &repo.host)?;
        let state = self
            .secure
            .borrow_mut()
            .as_mut()
            .ok_or(Error::Unavailable)?
            .load(&repo.id())?
            .unwrap_or(RepositoryState::new(&repo.trusted_root)?);
        let tls = if repo.tls_roots_der.is_empty() {
            if self.tls.0 == 0 {
                return Err(Error::Unavailable);
            }
            bexos_net::secure::RootConfigCache::new(self.tls)
                .config(&[b"h2", b"http/1.1"])
                .map_err(|_| Error::Unavailable)?
        } else {
            let mut roots = rustls::RootCertStore::empty();
            for root in &repo.tls_roots_der {
                roots
                    .add(rustls::pki_types::CertificateDer::from(root.clone()))
                    .map_err(|_| Error::VerifyFailed)?;
            }
            let mut config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Arc::new(config)
        };
        let token = self
            .vault
            .get(&repo.host)
            .filter(|credential| !credential.token.bytes().is_empty())
            .map(|credential| Secret::new(credential.token.bytes().to_vec()));
        let identity = if let Some(credential) = self
            .vault
            .get(&repo.host)
            .filter(|credential| !credential.private_key.bytes().is_empty())
        {
            let mut authenticated = (*tls).clone();
            let key =
                rustls::pki_types::PrivateKeyDer::try_from(credential.private_key.bytes().to_vec())
                    .map_err(|_| Error::InvalidArgs)?;
            let signing = rustls::crypto::ring::sign::any_supported_type(&key)
                .map_err(|_| Error::InvalidArgs)?;
            let certified = rustls::sign::CertifiedKey::new(
                credential
                    .certificates
                    .iter()
                    .cloned()
                    .map(rustls::pki_types::CertificateDer::from)
                    .collect(),
                signing,
            );
            certified.keys_match().map_err(|_| Error::InvalidArgs)?;
            authenticated.client_auth_cert_resolver =
                Arc::new(crate::transport::ClientIdentity(Arc::new(certified)));
            Some((repo.host.clone(), Arc::new(authenticated)))
        } else {
            None
        };
        let transport = LiveTransport {
            directory: self.directory,
            connect_timeout_ms: self.config.connect_timeout_ms,
            request_timeout_ms: self.config.request_timeout_ms,
            tls,
            identity,
        };
        let mut oci = Oci {
            transport,
            repository: repo,
            token,
            transfers: self.transfers.clone(),
            credential_generation: self
                .vault
                .get(&query.registry_host)
                .map_or(0, |c| c.generation),
        };
        let config = self.config.clone();
        let query = query.clone();
        let cas = self.cas.clone();
        let secure = self.secure.clone();
        let repository_id = oci.repository.id();
        Ok(Box::pin(async move {
            if let Some(digest) = source_digest {
                return resolution::download_known(&config, &query, digest, &mut oci, state).await;
            }
            resolution::download(
                &config,
                &query,
                now,
                &mut oci,
                state,
                |digest| cas.borrow_mut().as_mut().ok_or(Error::Io)?.get(digest),
                |state| {
                    resolution::persist_transition(
                        secure.borrow_mut().as_mut().ok_or(Error::Unavailable)?,
                        &repository_id,
                        state,
                    )
                },
            )
            .await
        }))
    }
    fn valid_time(&self) -> Result<u64> {
        use time_fidl::{FidlDecode, FidlEncode};
        if self.time.0 == 0 {
            bexos_userspace::log("pkgd: trusted time service unavailable\n");
            return Err(Error::Unavailable);
        }
        let mut bytes = [0; 32];
        let encoded = time_fidl::TimeManagerGetTimeQualityRequest {}
            .encode(&mut bytes, &mut [])
            .map_err(|_| Error::Unavailable)?;
        let message = bexos_userspace::Rpc(self.time)
            .call_raw(1, &bytes[..encoded.bytes], &[], true)
            .map_err(|_| Error::Unavailable)?;
        let response = time_fidl::TimeManagerGetTimeQualityResponse::decode(&message.bytes, &[])
            .map_err(|_| Error::Unavailable)?;
        if response.status != time_fidl::Status::Ok
            || response.quality.state != time_fidl::SyncState::Synced
            || !matches!(
                response.quality.source,
                time_fidl::ClockSource::RtcHardware | time_fidl::ClockSource::NtsSecure
            )
        {
            bexos_userspace::log(&format!(
                "pkgd: trusted time rejected status={:?} state={:?} source={:?} error={:?}\n",
                response.status,
                response.quality.state,
                response.quality.source,
                response.quality.last_error,
            ));
            return Err(Error::Unavailable);
        }
        bexos_userspace::clock::realtime_ns()
            .map(|now| now / 1_000_000_000)
            .map_err(|error| {
                bexos_userspace::log(&format!("pkgd: realtime clock unavailable {error:?}\n"));
                Error::Unavailable
            })
    }
    fn connect_services(&mut self) {
        if self.directory.0 == 0 {
            return;
        }
        let mut directory =
            bexos_userspace::service_directory::ServiceDirectoryClient::new(self.directory);
        for (slot, name, capability) in [
            (&mut self.network, "bexos.net.SocketProvider", "Public"),
            (
                &mut self.tls,
                "bexos.security.trust.TlsTrustManager",
                "Public",
            ),
            (&mut self.tee, "tee_manager", "PackageState"),
            (&mut self.time, "bexos.time.TimeManager", "Public"),
        ] {
            if slot.0 == 0 {
                if let Ok(channel) = directory.connect(name, capability) {
                    *slot = channel;
                }
            }
        }
    }
}
struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("pkgd startup");
    let mut runtime = if startup.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, startup.migration_generation)
            .expect("pkgd adoption")
    } else {
        let mut runtime = Runtime::empty();
        runtime.control = control;
        runtime.migration = startup.migration;
        for grant in startup.service_grants {
            let endpoint = Channel(grant.endpoint);
            match grant.protocol.as_str() {
                "VfsManager" => runtime.vfs = endpoint,
                "SocketProvider" => runtime.network = endpoint,
                "Netstack" if runtime.network.0 == 0 => runtime.network = endpoint,
                "TlsTrustManager" => runtime.tls = endpoint,
                "TeeManager" => runtime.tee = endpoint,
                "TimeManager" => runtime.time = endpoint,
                "bexos.app.service_directory.ServiceDirectory" => runtime.directory = endpoint,
                _ => {
                    let _ = Memory::close(endpoint.0);
                }
            }
        }
        // STORAGE becomes usable after appd pivots. Resolver operations open it
        // lazily; readiness must not wait on a filesystem request back into boot.
        Startup::ready(control).expect("pkgd ready");
        runtime
    };
    let mut source = Source::new(runtime.migration);
    let mut paused = false;
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.active() || source.quiescing() {
            runtime.pause();
            paused = true;
            bexos_userspace::yield_now();
            continue;
        }
        if paused {
            let _ = runtime.open_storage();
            paused = false;
        }
        if let Ok(message) = runtime.control.try_recv() {
            if message.handles.len() == 1 {
                let endpoint = message.handles[0];
                if let Some(binding) = core::str::from_utf8(&message.bytes)
                    .ok()
                    .and_then(ServiceBinding::parse)
                {
                    if let Some(package) = binding.caller_package {
                        if runtime.clients.len() < runtime.config.max_waiters
                            && matches!(
                                binding.protocol.as_str(),
                                "PackageResolver" | "CredentialManager"
                            )
                        {
                            runtime.clients.push(Client {
                                channel: endpoint,
                                package,
                                protocol: binding.protocol,
                                methods: binding.method_ordinals,
                            });
                            source.changed(0);
                        } else {
                            let _ = Memory::close(endpoint);
                        }
                    } else {
                        let _ = Memory::close(endpoint);
                    }
                } else {
                    let _ = Memory::close(endpoint);
                }
            } else {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
            }
        }
        if crate::wire::poll(&mut runtime) {
            source.changed_keys(runtime.state_keys());
        }
        runtime.pump();
        source.changed_keys(runtime.state_keys());
        bexos_userspace::yield_now();
    }
}
