use alloc::vec::Vec;
use bexos_i2c_spi::{
    BusController, BusKind, DeterministicBackend, Topology,
    migration::{decode_backend, decode_controller, encode_backend, encode_controller},
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub registry: Option<Channel>,
    pub controllers: Vec<BusController>,
    pub backend: DeterministicBackend,
    pub fixture_clients: Vec<BoundServiceEndpoint>,
    pub registered: bool,
}

impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        topology: Topology,
    ) -> Result<Self, ()> {
        let backend = DeterministicBackend::new(&topology);
        let controllers = topology
            .controllers
            .into_iter()
            .filter(|controller| controller.bus == BusKind::Spi)
            .map(BusController::new)
            .collect::<Vec<_>>();
        if controllers.is_empty() {
            return Err(());
        }
        Ok(Self {
            control,
            migration,
            registry: None,
            controllers,
            backend,
            fixture_clients: Vec::new(),
            registered: false,
        })
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            registry: None,
            controllers: Vec::new(),
            backend: DeterministicBackend::default(),
            fixture_clients: Vec::new(),
            registered: true,
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2, 3]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |channel| channel.0));
                w.word(self.registry.map_or(0, |channel| channel.0));
                w.word(self.registered as u64);
            }
            1 => {
                w.word(1);
                w.word(self.controllers.len() as u64);
                for controller in &self.controllers {
                    encode_controller(&mut w, controller);
                }
            }
            2 => encode_backend(&mut w, &self.backend),
            3 => {
                w.word(1);
                w.word(self.fixture_clients.len() as u64);
                for client in &self.fixture_clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            0 => {
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                let registry = r.word()?;
                self.registry = (registry != 0).then_some(Channel(registry));
                self.registered = r.flag()?;
            }
            1 => {
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.controllers.clear();
                for _ in 0..r.count(32)? {
                    self.controllers.push(decode_controller(&mut r)?);
                }
            }
            2 => self.backend = decode_backend(&mut r)?,
            3 => {
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.fixture_clients.clear();
                for _ in 0..r.count(16)? {
                    let channel = Channel(r.word()?);
                    let mut methods = Vec::new();
                    for _ in 0..r.count(16)? {
                        methods.push(r.word()?);
                    }
                    self.fixture_clients
                        .push(BoundServiceEndpoint::new(channel, methods));
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.controllers.is_empty() {
            return Err(Error::InvalidData);
        }
        for controller in &self.controllers {
            Topology {
                controllers: alloc::vec![controller.config.clone()],
            }
            .validate()
            .map_err(|_| Error::InvalidData)?;
        }
        Ok(())
    }

    fn quiescence_ready(&self) -> bool {
        self.controllers
            .iter()
            .all(|controller| controller.active.is_none())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.registry {
            out.push(Resource::Handle(channel.0));
        }
        for controller in &self.controllers {
            for client in &controller.clients {
                out.push(Resource::Handle(client.channel));
            }
        }
        for client in &self.fixture_clients {
            out.push(Resource::Handle(client.channel.0));
        }
        out
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!("spid: adopted generation={generation}\n"));
    }
}
