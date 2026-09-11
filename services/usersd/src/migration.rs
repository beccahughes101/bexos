use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;

use crate::auth::{RuntimeUserAuthProvider, TeeUserAuthProvider, UserAuthProvider};
use crate::service::UsersdService;
use bexos_trusty_client::users::HardwareAuthToken;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub vfsd: Channel,
    pub service: UsersdService,
    pub auth: RuntimeUserAuthProvider,
    pub generation: u64,
    pub unlocked: Vec<u64>,
    pub auth_tokens: Vec<HardwareAuthToken>,
    pub watchers: Vec<u64>,
    pub clients: Vec<BoundServiceEndpoint>,
}

impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        vfsd: Channel,
        service: UsersdService,
        auth: RuntimeUserAuthProvider,
        generation: u64,
    ) -> Self {
        Self {
            control,
            migration,
            vfsd,
            service,
            auth,
            generation,
            unlocked: Vec::new(),
            auth_tokens: Vec::new(),
            watchers: Vec::new(),
            clients: Vec::new(),
        }
    }

    /// Select tokens that are still valid at the source's secure time without
    /// mutating the source cache. This keeps aborted handovers retryable and
    /// preserves each token's original timestamp and deadline.
    pub fn migratable_auth_tokens_at(&self, now_ms: u64) -> Vec<&HardwareAuthToken> {
        self.auth_tokens
            .iter()
            .filter(|token| token.is_well_formed() && token.is_valid_at(now_ms))
            .collect()
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            vfsd: Channel(0),
            service: UsersdService::new(),
            auth: RuntimeUserAuthProvider::Unsupported,
            generation: 0,
            unlocked: Vec::new(),
            auth_tokens: Vec::new(),
            watchers: Vec::new(),
            clients: Vec::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2, 3, 4, 5]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        match key {
            0 => {
                let mut w = Encoder::new();
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map(|c| c.0).unwrap_or(0));
                w.word(self.vfsd.0);
                w.word(self.generation);
                Ok(Some(w.finish()))
            }
            1 => {
                let mut w = Encoder::new();
                w.word(self.unlocked.len() as u64);
                for uid in &self.unlocked {
                    w.word(*uid);
                }
                Ok(Some(w.finish()))
            }
            2 => {
                let mut w = Encoder::new();
                w.word(self.watchers.len() as u64);
                for watcher in &self.watchers {
                    w.word(*watcher);
                }
                Ok(Some(w.finish()))
            }
            3 => {
                let mut w = Encoder::new();
                w.word(1);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                Ok(Some(w.finish()))
            }
            4 => {
                let now_ms = self.auth.now_ms();
                let migratable = if now_ms == 0 {
                    // The unsupported provider is used only by deterministic
                    // host tests; production usersd always has secure time.
                    // Malformed tokens are never eligible for handover.
                    self.auth_tokens
                        .iter()
                        .filter(|token| token.is_well_formed())
                        .collect::<Vec<_>>()
                } else {
                    self.migratable_auth_tokens_at(now_ms)
                };
                let mut w = Encoder::new();
                w.word(1);
                w.word(migratable.len() as u64);
                for token in migratable {
                    w.word(token.uid);
                    w.word(token.secure_user_id);
                    w.word(token.secure_timestamp_ms);
                    w.word(token.expires_at_ms);
                    w.word(token.encoded.len() as u64);
                    for byte in &token.encoded {
                        w.word(u64::from(*byte));
                    }
                }
                Ok(Some(w.finish()))
            }
            5 => {
                let mut w = Encoder::new();
                match self.auth {
                    RuntimeUserAuthProvider::Unsupported => {
                        w.word(0);
                        w.word(0);
                        w.word(0);
                        w.word(0);
                    }
                    RuntimeUserAuthProvider::Tee(provider) => {
                        w.word(1);
                        w.word(provider.client().0);
                        w.word(provider.gatekeeper_session().unwrap_or(0));
                        w.word(provider.keymint_session().unwrap_or(0));
                    }
                }
                Ok(Some(w.finish()))
            }
            _ => Ok(None),
        }
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let bytes = bytes.ok_or(Error::InvalidData)?;
        match key {
            0 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.vfsd = Channel(r.word()?);
                self.generation = r.word()?;
                r.finish()?;
            }
            1 => {
                let mut r = Decoder::new(bytes);
                let count = r.word()?;
                self.unlocked.clear();
                for _ in 0..count {
                    self.unlocked.push(r.word()?);
                }
                r.finish()?;
            }
            2 => {
                let mut r = Decoder::new(bytes);
                self.watchers.clear();
                for _ in 0..r.count(64)? {
                    self.watchers.push(r.word()?);
                }
                r.finish()?;
            }
            3 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.clients.clear();
                for _ in 0..r.count(1024)? {
                    let channel = Channel(r.word()?);
                    let mut allowed_methods = Vec::new();
                    for _ in 0..r.count(128)? {
                        allowed_methods.push(r.word()?);
                    }
                    self.clients
                        .push(BoundServiceEndpoint::new(channel, allowed_methods));
                }
                r.finish()?;
            }
            4 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                let mut auth_tokens = Vec::new();
                for _ in 0..r.count(64)? {
                    let uid = r.word()?;
                    let secure_user_id = r.word()?;
                    let secure_timestamp_ms = r.word()?;
                    let expires_at_ms = r.word()?;
                    let mut encoded = Vec::new();
                    for _ in 0..r.count(512)? {
                        let byte = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                        encoded.push(byte);
                    }
                    let token = HardwareAuthToken {
                        uid,
                        secure_user_id,
                        secure_timestamp_ms,
                        expires_at_ms,
                        encoded,
                    };
                    if !token.is_well_formed() {
                        return Err(Error::InvalidData);
                    }
                    auth_tokens.push(token);
                }
                r.finish()?;
                self.auth_tokens = auth_tokens;
            }
            5 => {
                let mut r = Decoder::new(bytes);
                let kind = r.word()?;
                let client = r.word()?;
                let gatekeeper = r.word()?;
                let keymint = r.word()?;
                self.auth = if kind == 1 && client != 0 {
                    RuntimeUserAuthProvider::Tee(TeeUserAuthProvider::from_parts(
                        Channel(client),
                        (gatekeeper != 0).then_some(gatekeeper),
                        (keymint != 0).then_some(keymint),
                    ))
                } else {
                    RuntimeUserAuthProvider::Unsupported
                };
                r.finish()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.vfsd.0 == 0 {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.validate()?;
        let now_ms = self.auth.now_ms();
        if now_ms != 0 {
            self.auth_tokens
                .retain(|token| token.is_well_formed() && token.is_valid_at(now_ms));
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = alloc::vec![self.control.0, self.vfsd.0];
        if let Some(migration) = self.migration {
            handles.push(migration.0);
        }
        handles.extend(self.watchers.iter().copied());
        handles.extend(self.clients.iter().map(|client| client.channel.0));
        if let RuntimeUserAuthProvider::Tee(provider) = self.auth {
            handles.push(provider.client().0);
        }
        handles.into_iter().map(Resource::Handle).collect()
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
    }
}
