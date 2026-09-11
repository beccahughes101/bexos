use super::*;
use bexos_migration::{
    Error, blob,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::Resource;
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub providers: Vec<u64>,
    pub session: Option<ProviderSession>,
    pub output: Vec<u8>,
    pub input: Vec<u8>,
    pub errors: Vec<u8>,
    pub eof: bool,
}
impl State for Runtime {
    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!(
            "shell-fixture: adopted generation={generation}\n"
        ));
    }
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            providers: Vec::new(),
            session: None,
            output: Vec::new(),
            input: Vec::new(),
            errors: Vec::new(),
            eof: false,
        }
    }
    fn keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        for (i, buffer) in [&self.output, &self.input, &self.errors]
            .into_iter()
            .enumerate()
        {
            keys.extend(blob::keys(i as u64 + 1, buffer));
        }
        keys
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            let buffer = match key >> 32 {
                1 => &self.output,
                2 => &self.input,
                3 => &self.errors,
                _ => return Err(Error::InvalidData),
            };
            return Ok(blob::record(Some(buffer), key));
        }
        let mut w = Encoder::new();
        w.word(3);
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.providers.len() as u64);
        for h in &self.providers {
            w.word(*h);
        }
        w.word(self.session.is_some() as u64);
        if let Some(s) = &self.session {
            s.encode(&mut w);
        }
        w.word(self.eof as u64);
        for buffer in [&self.output, &self.input, &self.errors] {
            w.word(buffer.len() as u64);
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            let buffer = match key >> 32 {
                1 => &mut self.output,
                2 => &mut self.input,
                3 => &mut self.errors,
                _ => return Err(Error::InvalidData),
            };
            return blob::adopt(Some(buffer), key, bytes);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 3 {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(r.word()?);
        self.migration = Some(Channel(r.word()?));
        self.providers.clear();
        for _ in 0..r.count(64)? {
            self.providers.push(r.word()?);
        }
        self.session = if r.flag()? {
            Some(ProviderSession::decode(&mut r)?)
        } else {
            None
        };
        self.eof = r.flag()?;
        for (buffer, max) in [
            (&mut self.output, 32768),
            (&mut self.input, 4096),
            (&mut self.errors, 4096),
        ] {
            let len = r.count(max)?;
            buffer.resize(len, 0);
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none_or(|c| c.0 == 0) {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
    fn resources(&self) -> Vec<Resource> {
        let mut h = vec![self.control.0];
        if let Some(c) = self.migration {
            h.push(c.0);
        }
        h.extend(&self.providers);
        if let Some(s) = &self.session {
            h.extend(s.handles());
        }
        h.into_iter().map(Resource::Handle).collect()
    }
}
