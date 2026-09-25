use bexos_component::{
    Channel,
    live_migration::{Resource, State},
    migration_codec::{
        Error,
        codec::{Decoder, Encoder},
    },
};

pub struct FixtureState {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub preserved_handle: Option<u64>,
    pub requests: u64,
}

impl FixtureState {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        preserved_handle: Option<u64>,
    ) -> Self {
        Self {
            control,
            migration,
            preserved_handle,
            requests: 1,
        }
    }
}

impl State for FixtureState {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            preserved_handle: None,
            requests: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut encoder = Encoder::new();
        encoder.word(1);
        encoder.word(self.control.0);
        encoder.word(self.migration.map_or(0, |channel| channel.0));
        encoder.word(self.preserved_handle.unwrap_or(0));
        encoder.word(self.requests);
        Ok(Some(encoder.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut decoder = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if decoder.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(decoder.word()?);
        let migration = decoder.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        let preserved = decoder.word()?;
        self.preserved_handle = (preserved != 0).then_some(preserved);
        self.requests = decoder.word()?;
        decoder.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.requests == 0 {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            resources.push(Resource::Handle(channel.0));
        }
        if let Some(handle) = self.preserved_handle {
            resources.push(Resource::Handle(handle));
        }
        resources
    }

    fn activation_markers(&self) -> [u64; 3] {
        [1, self.requests, self.preserved_handle.is_some() as u64]
    }

    fn activated(&mut self, _generation: u64) {
        bexos_component::log("sdk-fixture: version-1 state and handles adopted\n");
    }
}

pub fn serve(mut state: FixtureState) -> ! {
    let mut source = bexos_component::live_migration::Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_component::migration::abort();
        }
        if !source.quiescing() {
            state.requests = state.requests.wrapping_add(1);
            source.changed(0);
        }
        bexos_component::yield_now();
    }
}
