use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};

impl State for Runtime {
    fn empty() -> Self {
        Self {
            manager: Channel(0),
            migration: None,
            initialized: false,
            endpoints: Vec::new(),
            requests: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.manager.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.requests);
        w.word(self.initialized as u64);
        w.word(self.endpoints.len() as u64);
        for endpoint in &self.endpoints {
            w.word(endpoint.channel.0);
            w.word(endpoint.allowed_methods.len() as u64);
            for ordinal in &endpoint.allowed_methods {
                w.word(*ordinal);
            }
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        self.manager = Channel(r.word()?);
        self.migration = nonzero_channel(r.word()?);
        self.requests = r.word()?;
        self.initialized = r.flag()?;
        self.endpoints.clear();
        for _ in 0..r.count(64)? {
            let channel = Channel(r.word()?);
            let mut allowed = Vec::new();
            for _ in 0..r.count(64)? {
                allowed.push(r.word()?);
            }
            self.endpoints
                .push(BoundServiceEndpoint::new(channel, allowed));
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.manager.0 == 0 || self.migration.is_none() || !self.initialized {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.manager.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.endpoints
                .iter()
                .map(|endpoint| Resource::Handle(endpoint.channel.0)),
        );
        out
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!("cmos: adopted generation={generation}\n"));
    }
}

fn nonzero_channel(raw: u64) -> Option<Channel> {
    (raw != 0).then_some(Channel(raw))
}
