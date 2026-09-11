use alloc::vec;
use alloc::vec::Vec;
use bexos_lazy_service::{GuardedServiceEndpoint, KeepAlive, LazyServiceController};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;

use crate::service::{
    KeychainService, RuntimeHardwareKeyProvider, TeeKeyMintClient, UnsupportedHardwareKeyProvider,
};

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub users: Channel,
    pub user_auth: Channel,
    pub lazy: LazyServiceController,
    pub(crate) volatile_keep_alive: Option<KeepAlive>,
    pub(crate) idle_stop_requested: Option<u64>,
    pub service: KeychainService<RuntimeHardwareKeyProvider>,
    pub(crate) clients: Vec<GuardedServiceEndpoint>,
    pub(crate) store_chunks: Option<Vec<Option<Vec<u8>>>>,
    store_len: usize,
    adopted_metadata: u8,
}

impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        users: Channel,
        user_auth: Channel,
        service: KeychainService<RuntimeHardwareKeyProvider>,
        lazy: LazyServiceController,
        volatile_keep_alive: Option<KeepAlive>,
    ) -> Self {
        Self {
            control,
            migration,
            users,
            user_auth,
            lazy,
            volatile_keep_alive,
            idle_stop_requested: None,
            service,
            clients: Vec::new(),
            store_chunks: None,
            store_len: 0,
            adopted_metadata: 0,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            users: Channel(0),
            user_auth: Channel(0),
            clients: Vec::new(),
            lazy: LazyServiceController::new(0),
            volatile_keep_alive: None,
            idle_stop_requested: None,
            store_chunks: None,
            store_len: 0,
            adopted_metadata: 0,
            service: KeychainService::with_hardware(RuntimeHardwareKeyProvider::Unsupported(
                UnsupportedHardwareKeyProvider,
            )),
        }
    }

    fn keys(&self) -> Vec<u64> {
        let len = self.service.checkpoint_stores().len();
        (0..3 + len.div_ceil(16 * 1024) as u64).collect()
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key >= 3 {
            let snapshot = self.service.checkpoint_stores();
            return Ok(snapshot
                .chunks(16 * 1024)
                .nth((key - 3) as usize)
                .map(<[u8]>::to_vec));
        }
        match key {
            0 => {
                let mut w = Encoder::new();
                w.word(4);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.service.checkpoint_stores().len() as u64);
                w.word(self.control.0);
                w.word(self.migration.map(|c| c.0).unwrap_or(0));
                w.word(self.users.0);
                w.word(self.user_auth.0);
                #[cfg(feature = "persistent")]
                w.word(self.service.vfsd.map(|c| c.0).unwrap_or(0));
                #[cfg(not(feature = "persistent"))]
                w.word(0);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.endpoint.channel.0);
                    w.word(client.endpoint.allowed_methods.len() as u64);
                    for method in &client.endpoint.allowed_methods {
                        w.word(*method);
                    }
                }
                w.bytes(&bexos_lazy_service::migration::encode_snapshot(
                    self.lazy.snapshot(),
                ));
                w.word(self.volatile_keep_alive.is_some() as u64);
                match self.idle_stop_requested {
                    Some(generation) => {
                        w.word(1);
                        w.word(generation);
                    }
                    None => {
                        w.word(0);
                        w.word(0);
                    }
                }
                Ok(Some(w.finish()))
            }
            1 => {
                let mut w = Encoder::new();
                match self.service.hardware {
                    RuntimeHardwareKeyProvider::Unsupported(_) => {
                        w.word(0);
                        w.word(0);
                        w.word(0);
                    }
                    RuntimeHardwareKeyProvider::Tee(client) => {
                        w.word(1);
                        w.word(client.client().0);
                        w.word(client.cached_session_id().unwrap_or(0));
                    }
                }
                Ok(Some(w.finish()))
            }
            2 => {
                let uids = self.service.user_uids();
                let mut w = Encoder::new();
                w.word(uids.len() as u64);
                for uid in uids {
                    w.word(uid);
                }
                Ok(Some(w.finish()))
            }
            _ => Ok(None),
        }
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key >= 3 {
            if let Some(chunks) = self.store_chunks.as_mut() {
                let index = usize::try_from(key - 3).map_err(|_| Error::InvalidData)?;
                if index >= chunks.len() {
                    return if bytes.is_none() {
                        Ok(())
                    } else {
                        Err(Error::InvalidData)
                    };
                }
                if bytes.is_some_and(|bytes| bytes.len() > 16 * 1024) {
                    return Err(Error::Capacity);
                }
                chunks[index] = bytes.map(<[u8]>::to_vec);
                return Ok(());
            }
        }
        let bytes = bytes.ok_or(Error::InvalidData)?;
        match key {
            0 => {
                let mut r = Decoder::new(bytes);
                let version = r.word()?;
                if version == 3 || version == 4 {
                    if r.word()? != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                        return Err(Error::InvalidData);
                    }
                    let len = r.count(8 * 1024 * 1024)?;
                    if len == 0 {
                        return Err(Error::InvalidData);
                    }
                    self.store_len = len;
                    self.store_chunks
                        .get_or_insert_with(Vec::new)
                        .resize_with(len.div_ceil(16 * 1024), || None);
                } else if version != 2 || cfg!(bexos_arch_x86_64) {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.users = Channel(r.word()?);
                self.user_auth = Channel(r.word()?);
                let _vfsd = r.word()?;
                #[cfg(feature = "persistent")]
                {
                    self.service.vfsd = (_vfsd != 0).then_some(Channel(_vfsd));
                }
                if version == 3 || version == 4 {
                    let mut restored_clients = Vec::new();
                    let client_count = r.count(128)?;
                    for _ in 0..client_count {
                        let channel = Channel(r.word()?);
                        let mut methods = Vec::new();
                        for _ in 0..r.count(7)? {
                            methods.push(r.word()?);
                        }
                        restored_clients.push(BoundServiceEndpoint::new(channel, methods));
                    }
                    if version == 4 {
                        self.lazy = LazyServiceController::restore(
                            bexos_lazy_service::migration::decode_snapshot(r.bytes(256)?)?,
                        );
                        self.volatile_keep_alive =
                            r.flag()?.then(|| self.lazy.restore_keep_alive());
                        self.idle_stop_requested = if r.flag()? {
                            Some(r.word()?)
                        } else {
                            let _ = r.word()?;
                            None
                        };
                    } else {
                        self.lazy = LazyServiceController::new(0);
                        for _ in 0..client_count {
                            core::mem::forget(self.lazy.track_connection());
                        }
                    }
                    self.clients = restored_clients
                        .into_iter()
                        .map(|endpoint| {
                            GuardedServiceEndpoint::from_existing(
                                endpoint,
                                self.lazy.restore_connection_guard(),
                            )
                        })
                        .collect();
                }
                r.finish()?;
            }
            1 => {
                let mut r = Decoder::new(bytes);
                let kind = r.word()?;
                let client = r.word()?;
                let session = r.word()?;
                self.service.hardware = if kind == 1 && client != 0 {
                    RuntimeHardwareKeyProvider::Tee(TeeKeyMintClient::from_parts(
                        Channel(client),
                        (session != 0).then_some(session),
                    ))
                } else if kind == 0 && client == 0 && session == 0 {
                    RuntimeHardwareKeyProvider::Unsupported(UnsupportedHardwareKeyProvider)
                } else {
                    return Err(Error::InvalidData);
                };
                r.finish()?;
                self.adopted_metadata |= 1;
            }
            2 => {
                let mut r = Decoder::new(bytes);
                let mut uids = Vec::new();
                for _ in 0..r.count(4096)? {
                    let uid = r.word()?;
                    if uid == 0 || uids.contains(&uid) {
                        return Err(Error::InvalidData);
                    }
                    uids.push(uid);
                }
                r.finish()?;
                self.adopted_metadata |= 2;
                #[cfg(feature = "persistent")]
                if self.store_chunks.is_none() {
                    self.service.adopt_legacy_stores(&uids)?;
                }
            }
            3 => {
                if !bytes.is_empty() {
                    return Err(Error::InvalidData);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.users.0 == 0
            || self.user_auth.0 == 0
            || self.user_auth.0 == self.users.0
        {
            return Err(Error::InvalidData);
        }
        if self.clients.len() > 128
            || self.clients.iter().enumerate().any(|(i, client)| {
                client.endpoint.channel.0 == 0
                    || client.endpoint.channel.0 == self.control.0
                    || self.clients[..i]
                        .iter()
                        .any(|old| old.endpoint.channel.0 == client.endpoint.channel.0)
                    || client
                        .endpoint
                        .allowed_methods
                        .iter()
                        .any(|method| !(1..=7).contains(method))
                    || client.endpoint.allowed_methods.len() > 7
            })
        {
            return Err(Error::InvalidData);
        }
        let bytes = self.snapshot_bytes()?;
        self.service.validate_stores(&bytes)
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = vec![self.control.0, self.users.0, self.user_auth.0];
        if let Some(migration) = self.migration {
            handles.push(migration.0);
        }
        #[cfg(feature = "persistent")]
        if let Some(vfsd) = self.service.vfsd {
            handles.push(vfsd.0);
        }
        if let RuntimeHardwareKeyProvider::Tee(client) = self.service.hardware {
            handles.push(client.client().0);
        }
        handles.extend(self.clients.iter().map(|client| client.endpoint.channel.0));
        handles
            .into_iter()
            .filter(|handle| *handle != 0)
            .map(Resource::Handle)
            .collect()
    }

    fn activated(&mut self, _generation: u64) {
        if self.store_chunks.is_some() {
            let bytes = self.snapshot_bytes().expect("validated keychain snapshot");
            self.service
                .adopt_stores(&bytes)
                .expect("validated keychain stores");
            self.store_chunks = None;
        }
    }
}

impl Runtime {
    fn snapshot_bytes(&self) -> Result<Vec<u8>, Error> {
        if let Some(chunks) = &self.store_chunks {
            if self.adopted_metadata != 3 {
                return Err(Error::InvalidData);
            }
            let mut bytes = Vec::new();
            for (index, chunk) in chunks.iter().enumerate() {
                let chunk = chunk.as_deref().ok_or(Error::InvalidData)?;
                let expected = (self.store_len - index * 16 * 1024).min(16 * 1024);
                if chunk.len() != expected {
                    return Err(Error::InvalidData);
                }
                bytes.extend_from_slice(chunk);
            }
            Ok(bytes)
        } else {
            Ok(self.service.checkpoint_stores())
        }
    }
}
