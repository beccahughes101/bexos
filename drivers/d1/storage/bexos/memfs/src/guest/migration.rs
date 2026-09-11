use alloc::vec::Vec;

use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::{Channel, log};

use crate::guest::{Endpoint, TmpInstance};
use crate::{MemFs, OpenedNode};

pub const INSTANCE: u64 = 1 << 56;
pub const ENDPOINT: u64 = 2 << 56;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub(crate) instances: Vec<TmpInstance>,
    pub(crate) endpoints: Vec<Endpoint>,
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            instances: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        keys.extend((0..self.instances.len()).map(|index| INSTANCE | index as u64));
        keys.extend(
            self.endpoints
                .iter()
                .map(|endpoint| endpoint_key(endpoint.channel)),
        );
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key == 0 {
            let mut w = Encoder::new();
            w.word(1);
            w.word(self.control.0);
            w.word(self.migration.map_or(0, |c| c.0));
            w.word(self.instances.len() as u64);
            w.word(self.endpoints.len() as u64);
            return Ok(Some(w.finish()));
        }
        if key & INSTANCE == INSTANCE {
            let index = usize::try_from(key & !INSTANCE).map_err(|_| Error::InvalidData)?;
            return Ok(self
                .instances
                .get(index)
                .map(|instance| instance.fs.checkpoint()));
        }
        if key & ENDPOINT == ENDPOINT {
            let channel = key & !ENDPOINT;
            let Some(endpoint) = self
                .endpoints
                .iter()
                .find(|endpoint| endpoint.channel.0 == channel)
            else {
                return Ok(None);
            };
            let mut w = Encoder::new();
            w.word(1);
            w.word(endpoint.channel.0);
            w.word(endpoint.instance as u64);
            w.word(endpoint.inode);
            match &endpoint.opened {
                Ok(None) => w.word(0),
                Ok(Some(opened)) => {
                    w.word(1);
                    w.bytes(&opened.checkpoint());
                }
                Err(status) => {
                    w.word(2);
                    w.word(*status as i32 as u32 as u64);
                }
            }
            return Ok(Some(w.finish()));
        }
        Err(Error::InvalidData)
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 0 {
            let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
            if r.word()? != 1 {
                return Err(Error::UnsupportedVersion);
            }
            self.control = Channel(r.word()?);
            let migration = r.word()?;
            self.migration = (migration != 0).then_some(Channel(migration));
            self.instances = Vec::with_capacity(r.count(4096)?);
            self.endpoints = Vec::with_capacity(r.count(4096)?);
            return r.finish();
        }
        if key & INSTANCE == INSTANCE {
            let fs =
                MemFs::adopt(bytes.ok_or(Error::InvalidData)?).map_err(|_| Error::InvalidData)?;
            self.instances.push(TmpInstance { fs });
            return Ok(());
        }
        if key & ENDPOINT == ENDPOINT {
            let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
            if r.word()? != 1 {
                return Err(Error::UnsupportedVersion);
            }
            let channel = Channel(r.word()?);
            let instance = usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let inode = r.word()?;
            let opened = match r.word()? {
                0 => Ok(None),
                1 => Ok(Some(
                    OpenedNode::adopt(r.bytes(4096)?).map_err(|_| Error::InvalidData)?,
                )),
                2 => Err(fs_status(
                    u32::try_from(r.word()?).map_err(|_| Error::InvalidData)? as i32,
                )),
                _ => return Err(Error::InvalidData),
            };
            r.finish()?;
            self.endpoints.push(Endpoint {
                channel,
                instance,
                inode,
                opened,
            });
            return Ok(());
        }
        Err(Error::InvalidData)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        for endpoint in &self.endpoints {
            if endpoint.channel.0 == 0 || endpoint.instance >= self.instances.len() {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = alloc::vec![self.control.0];
        if let Some(migration) = self.migration {
            handles.push(migration.0);
        }
        handles.extend(self.endpoints.iter().map(|endpoint| endpoint.channel.0));
        handles.into_iter().map(Resource::Handle).collect()
    }

    fn activated(&mut self, generation: u64) {
        log(&alloc::format!(
            "memfs: adopted generation={generation}; tmp instances retained\n"
        ));
    }
}

pub fn endpoint_key(channel: Channel) -> u64 {
    ENDPOINT | channel.0
}

fn fs_status(status: i32) -> fs_fidl::FsStatus {
    match status {
        0 => fs_fidl::FsStatus::Ok,
        -1 => fs_fidl::FsStatus::NotFound,
        -2 => fs_fidl::FsStatus::NotDirectory,
        -3 => fs_fidl::FsStatus::IsDirectory,
        -4 => fs_fidl::FsStatus::NotEmpty,
        -5 => fs_fidl::FsStatus::NoSpace,
        -6 => fs_fidl::FsStatus::Io,
        -9 => fs_fidl::FsStatus::ReadOnly,
        -10 => fs_fidl::FsStatus::AccessDenied,
        -11 => fs_fidl::FsStatus::InvalidArgs,
        -12 => fs_fidl::FsStatus::AlreadyExists,
        -13 => fs_fidl::FsStatus::BadState,
        _ => fs_fidl::FsStatus::Io,
    }
}
