use bexos_component::{Channel, live_migration::{Resource, State}, migration_codec::{Error, codec::{Decoder, Encoder}}};

pub struct ExampleState { control: Channel, migration: Option<Channel>, handle: Option<u64>, counter: u64 }

impl ExampleState {
    pub fn new(control: Channel, migration: Option<Channel>, handle: Option<u64>) -> Self { Self { control, migration, handle, counter: 1 } }
}

impl State for ExampleState {
    fn empty() -> Self { Self { control: Channel(0), migration: None, handle: None, counter: 0 } }
    fn keys(&self) -> Vec<u64> { vec![0] }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 { return Err(Error::InvalidData); }
        let mut out = Encoder::new();
        out.word(1); out.word(self.control.0); out.word(self.migration.map_or(0, |value| value.0)); out.word(self.handle.unwrap_or(0)); out.word(self.counter);
        Ok(Some(out.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 { return Err(Error::InvalidData); }
        let mut input = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if input.word()? != 1 { return Err(Error::UnsupportedVersion); }
        self.control = Channel(input.word()?);
        let migration = input.word()?; self.migration = (migration != 0).then_some(Channel(migration));
        let handle = input.word()?; self.handle = (handle != 0).then_some(handle);
        self.counter = input.word()?; input.finish()
    }
    fn validate(&self) -> Result<(), Error> { if self.control.0 != 0 && self.migration.is_some() { Ok(()) } else { Err(Error::InvalidData) } }
    fn resources(&self) -> Vec<Resource> {
        let mut resources = vec![Resource::Handle(self.control.0)];
        if let Some(value) = self.migration { resources.push(Resource::Handle(value.0)); }
        if let Some(value) = self.handle { resources.push(Resource::Handle(value)); }
        resources
    }
    fn activated(&mut self, _generation: u64) { bexos_component::log("example: adopted state version 1\n"); }
}

pub fn serve(mut state: ExampleState) -> ! {
    let mut migration = bexos_component::live_migration::Source::new(state.migration);
    loop {
        let _ = migration.poll(&state);
        if !migration.quiescing() { state.counter = state.counter.wrapping_add(1); migration.changed(0); }
        bexos_component::yield_now();
    }
}
