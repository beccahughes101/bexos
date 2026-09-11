use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel, Memory,
    live_migration::{Resource, State},
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
pub trait Component: Default {
    fn quiescence_ready(&self) -> bool {
        true
    }
    fn record_keys(&self) -> Vec<u64> {
        vec![1]
    }
    fn encode_component_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 1 {
            return Ok(None);
        }
        let mut w = Encoder::new();
        self.encode(&mut w)?;
        Ok(Some(w.finish()))
    }
    fn adopt_component_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 1 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        self.decode(&mut r)?;
        r.finish()
    }
    fn finish_records(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn deadline_profile(&self) -> bool {
        false
    }
    fn frame_period_ns(&self) -> u64 {
        16_667_000
    }
    fn encode(&self, w: &mut Encoder) -> Result<(), Error>;
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error>;
    fn resources(&self) -> Vec<Resource>;
    fn activate(&mut self);
    fn validate(&self) -> Result<(), Error>;
}
pub struct Runtime<T: Component> {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub component: T,
}
impl<T: Component> Runtime<T> {
    pub fn new(control: Channel, migration: Option<Channel>, component: T) -> Self {
        Self {
            control,
            migration,
            clients: Vec::new(),
            component,
        }
    }
    pub fn bindings(&mut self, protocol: &str) -> bool {
        if let Ok(m) = self.control.try_recv() {
            if m.handles.len() == 1 {
                if let Ok(text) = core::str::from_utf8(&m.bytes) {
                    if let Some(b) = ServiceBinding::parse(text) {
                        if b.protocol_is(protocol) && self.clients.len() < 64 {
                            self.clients.push(BoundServiceEndpoint::new(
                                Channel(m.handles[0]),
                                b.method_ordinals,
                            ));
                            return true;
                        }
                    }
                }
            }
            for h in m.handles {
                let _ = Memory::close(h);
            }
        }
        false
    }
}
impl<T: Component> State for Runtime<T> {
    fn quiescence_ready(&self) -> bool {
        self.component.quiescence_ready()
    }
    fn empty() -> Self {
        Self::new(Channel(0), None, T::default())
    }
    fn keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        keys.extend(self.component.record_keys());
        keys
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        w.word(1);
        match key {
            0 => {
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |c| c.0));
                w.word(self.clients.len() as u64);
                for c in &self.clients {
                    w.word(c.channel.0);
                    w.word(c.allowed_methods.len() as u64);
                    for o in &c.allowed_methods {
                        w.word(*o);
                    }
                }
            }
            _ => {
                let Some(bytes) = self.component.encode_component_record(key)? else {
                    return Ok(None);
                };
                let mut out = w.finish();
                out.extend(bytes);
                return Ok(Some(out));
            }
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 && bytes.is_none() {
            return self.component.adopt_component_record(key, None);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        match key {
            0 => {
                let control = Channel(r.word()?);
                let migration = Some(Channel(r.word()?));
                let mut clients = Vec::new();
                for _ in 0..r.count(64)? {
                    let ch = Channel(r.word()?);
                    let mut ordinals = Vec::new();
                    for _ in 0..r.count(32)? {
                        ordinals.push(r.word()?);
                    }
                    clients.push(BoundServiceEndpoint::new(ch, ordinals));
                }
                r.finish()?;
                self.control = control;
                self.migration = migration;
                self.clients = clients;
                return Ok(());
            }
            _ => {
                return self
                    .component
                    .adopt_component_record(key, Some(&bytes.unwrap()[8..]));
            }
        }
    }
    // Bulk copy may contain chunks from different logical revisions. Assemble
    // only after the complete delta prefix arrives at the final boundary.
    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.component.finish_records()?;
        self.validate()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none_or(|c| c.0 == 0)
            || self.clients.iter().any(|c| c.channel.0 == 0)
        {
            return Err(Error::InvalidData);
        }
        self.component.validate()
    }
    fn resources(&self) -> Vec<Resource> {
        let mut r = vec![Resource::Handle(self.control.0)];
        if let Some(c) = self.migration {
            r.push(Resource::Handle(c.0));
        }
        r.extend(self.clients.iter().map(|c| Resource::Handle(c.channel.0)));
        r.extend(self.component.resources());
        r
    }
    fn activated(&mut self, _: u64) {
        self.component.activate();
        if self.component.deadline_profile() {
            let _ =
                crate::scheduling::request_period(self.control, self.component.frame_period_ns());
        }
    }
}
