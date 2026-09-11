use alloc::vec::Vec;

use bexos_migration::{
    Error, blob,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;

use crate::TrustdService;

pub const TLS_ROOTS: u64 = 1 << 56;
pub const APP_ROOTS: u64 = 2 << 56;
pub const DYNAMIC_STATE: u64 = 3 << 56;
const LOCAL: u64 = (1 << 56) - 1;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub service: TrustdService,
    pub app_roots_encoded: Vec<u8>,
    pub dynamic_state_encoded: Vec<u8>,
    pub app_clients: Vec<BoundServiceEndpoint>,
    pub tls_clients: Vec<BoundServiceEndpoint>,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>, service: TrustdService) -> Self {
        let app_roots_encoded = service.app_roots_bytes();
        let dynamic_state_encoded = service.dynamic_state_bytes();
        Self {
            control,
            migration,
            service,
            app_roots_encoded,
            dynamic_state_encoded,
            app_clients: Vec::new(),
            tls_clients: Vec::new(),
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            service: TrustdService::default(),
            app_roots_encoded: Vec::new(),
            dynamic_state_encoded: Vec::new(),
            app_clients: Vec::new(),
            tls_clients: Vec::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        keys.extend(blob::keys(0, &self.service.tls_roots_redb).map(|k| TLS_ROOTS | k));
        keys.extend(blob::keys(0, &self.app_roots_encoded).map(|k| APP_ROOTS | k));
        keys.extend(blob::keys(0, &self.dynamic_state_encoded).map(|k| DYNAMIC_STATE | k));
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        match key & !LOCAL {
            TLS_ROOTS => return Ok(blob::record(Some(&self.service.tls_roots_redb), key)),
            APP_ROOTS => return Ok(blob::record(Some(&self.app_roots_encoded), key)),
            DYNAMIC_STATE => return Ok(blob::record(Some(&self.dynamic_state_encoded), key)),
            0 if key == 0 => {}
            _ => return Err(Error::InvalidData),
        }
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.service.generation);
        w.word(self.service.tls_roots_redb.len() as u64);
        w.word(self.app_roots_encoded.len() as u64);
        w.word(self.dynamic_state_encoded.len() as u64);
        w.word(self.app_clients.len() as u64);
        for client in &self.app_clients {
            encode_bound_client(&mut w, client);
        }
        w.word(self.tls_clients.len() as u64);
        for client in &self.tls_clients {
            encode_bound_client(&mut w, client);
        }
        w.word(u64::from(self.service.prod_template_disabled));
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        match key & !LOCAL {
            TLS_ROOTS => return blob::adopt(Some(&mut self.service.tls_roots_redb), key, bytes),
            APP_ROOTS => {
                blob::adopt(Some(&mut self.app_roots_encoded), key, bytes)?;
                if self
                    .service
                    .replace_app_roots_from_bytes(&self.app_roots_encoded)
                    .is_err()
                {
                    self.service.app_roots.clear();
                }
                return Ok(());
            }
            DYNAMIC_STATE => {
                blob::adopt(Some(&mut self.dynamic_state_encoded), key, bytes)?;
                if self
                    .service
                    .replace_dynamic_state_from_bytes(&self.dynamic_state_encoded)
                    .is_err()
                {
                    self.service.enterprise_roots.clear();
                    self.service.revocation = None;
                }
                return Ok(());
            }
            0 if key == 0 => {}
            _ => return Err(Error::InvalidData),
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 2 {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(r.word()?);
        self.migration = Some(Channel(r.word()?));
        self.service.generation = r.word()?;
        self.service
            .tls_roots_redb
            .resize(r.count(16 * 1024 * 1024)?, 0);
        self.app_roots_encoded.resize(r.count(1024 * 1024)?, 0);
        self.dynamic_state_encoded.resize(r.count(1024 * 1024)?, 0);
        self.app_clients.clear();
        for _ in 0..r.count(64)? {
            self.app_clients.push(decode_bound_client(&mut r)?);
        }
        self.tls_clients.clear();
        for _ in 0..r.count(64)? {
            self.tls_clients.push(decode_bound_client(&mut r)?);
        }
        self.service.prod_template_disabled = r.word()? != 0;
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.service.generation == 0 {
            return Err(Error::InvalidData);
        }
        let mut service = self.service.clone();
        service
            .replace_app_roots_from_bytes(&self.app_roots_encoded)
            .map_err(|_| Error::InvalidData)?;
        service
            .replace_dynamic_state_from_bytes(&self.dynamic_state_encoded)
            .map_err(|_| Error::InvalidData)?;
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.app_clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out.extend(
            self.tls_clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out
    }

    fn activated(&mut self, generation: u64) {
        self.service.generation = generation;
        bexos_userspace::log(&alloc::format!(
            "trustd: adopted generation={generation}; root stores and clients retained\n"
        ));
    }
}

fn encode_bound_client(w: &mut Encoder, client: &BoundServiceEndpoint) {
    w.word(client.channel.0);
    w.word(client.allowed_methods.len() as u64);
    for ordinal in &client.allowed_methods {
        w.word(*ordinal);
    }
}

fn decode_bound_client(r: &mut Decoder<'_>) -> Result<BoundServiceEndpoint, Error> {
    let channel = Channel(r.word()?);
    let mut methods = Vec::new();
    for _ in 0..r.count(32)? {
        methods.push(r.word()?);
    }
    Ok(BoundServiceEndpoint::new(channel, methods))
}
