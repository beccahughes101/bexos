use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
pub const ENDPOINT: u64 = 1 << 63;
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub archives: Vec<MountedArchive>,
    pub endpoints: Vec<Endpoint>,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            archives: Vec::new(),
            endpoints: Vec::new(),
        }
    }
    fn migration_state_limit() -> usize {
        32 * 1024 * 1024
    }
    fn keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        for (i, a) in self.archives.iter().enumerate() {
            keys.extend(
                a.fs.checkpoint_keys()
                    .into_iter()
                    .map(|k| ((i as u64 + 1) << 40) | k),
            );
        }
        keys.extend(self.endpoints.iter().map(|e| ENDPOINT | e.channel.0));
        keys
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        if key == 0 {
            w.word(1);
            w.word(self.control.0);
            w.word(self.migration.map_or(0, |c| c.0));
            w.word(self.archives.len() as u64);
        } else if key & ENDPOINT != 0 {
            let Some(e) = self
                .endpoints
                .iter()
                .find(|e| e.channel.0 == key & !ENDPOINT)
            else {
                return Ok(None);
            };
            w.word(e.channel.0);
            w.word(e.archive as u64);
            w.word(e.inode);
            match &e.file {
                Ok(None) => w.word(0),
                Ok(Some(f)) => {
                    w.word(1);
                    f.checkpoint(&mut w);
                }
                Err(e) => {
                    w.word(2);
                    w.word(*e as i32 as i64 as u64);
                }
            }
        } else {
            let id = (key >> 40).checked_sub(1).ok_or(Error::InvalidData)? as usize;
            return self
                .archives
                .get(id)
                .ok_or(Error::InvalidData)?
                .fs
                .checkpoint_record(key & ((1 << 40) - 1));
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key & ENDPOINT != 0 {
            self.endpoints.retain(|e| e.channel.0 != key & !ENDPOINT);
            let Some(bytes) = bytes else {
                return Ok(());
            };
            let mut r = Decoder::new(bytes);
            let channel = Channel(r.word()?);
            if channel.0 != key & !ENDPOINT {
                return Err(Error::InvalidData);
            }
            let archive = r.count(1024)?;
            let inode = r.word()?;
            let file = match r.word()? {
                0 => Ok(None),
                1 => Ok(Some(FileHandle::adopt(&mut r)?)),
                2 => Err(bexos_userspace::checkpoint::fs_status(r.word()?)?),
                _ => return Err(Error::InvalidData),
            };
            r.finish()?;
            self.endpoints.push(Endpoint {
                channel,
                archive,
                inode,
                file,
            });
            return Ok(());
        }
        let bytes = bytes.ok_or(Error::InvalidData)?;
        if key == 0 {
            let mut r = Decoder::new(bytes);
            if r.word()? != 1 {
                return Err(Error::UnsupportedVersion);
            }
            self.control = Channel(r.word()?);
            self.migration = Some(Channel(r.word()?));
            let count = r.count(1024)?;
            r.finish()?;
            while self.archives.len() < count {
                self.archives.push(MountedArchive {
                    fs: ArchiveFs::empty_checkpoint(),
                });
            }
            self.archives.truncate(count);
        } else {
            let id = (key >> 40).checked_sub(1).ok_or(Error::InvalidData)? as usize;
            self.archives
                .get_mut(id)
                .ok_or(Error::InvalidData)?
                .fs
                .adopt_record(key & ((1 << 40) - 1), Some(bytes))?;
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        for a in &self.archives {
            a.fs.validate_checkpoint()?;
        }
        for e in &self.endpoints {
            let fs = &self.archives.get(e.archive).ok_or(Error::InvalidData)?.fs;
            fs.attributes(e.inode).map_err(|_| Error::InvalidData)?;
            if e.file
                .as_ref()
                .ok()
                .and_then(|f| f.as_ref())
                .is_some_and(|f| f.inode() != e.inode)
            {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        core::iter::once(self.control.0)
            .chain(self.migration.map(|c| c.0))
            .chain(self.endpoints.iter().map(|e| e.channel.0))
            .map(Resource::Handle)
            .collect()
    }
    fn activated(&mut self, generation: u64) {
        log(&alloc::format!(
            "archivefs: adopted generation={generation}; nodes and cursors retained\n"
        ));
    }
}
