use super::*;
use crate::guest_block::SharedBlock;
use crate::guest_block::{Partition, VolumeDevice};
use crate::key::LockedVolumeKey;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
pub const ENDPOINT: u64 = 1 << 63;
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub volumes: Vec<Volume>,
    pub endpoints: Vec<Endpoint>,
    pub controls: Vec<MountControl>,
    pub expected_volumes: usize,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            volumes: Vec::new(),
            endpoints: Vec::new(),
            controls: Vec::new(),
            expected_volumes: 0,
        }
    }
    fn migration_state_limit() -> usize {
        // The mounted system namespace includes the disk-backed package image,
        // which is larger than the conservative default used by small drivers.
        64 * 1024 * 1024
    }
    fn keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        for (i, a) in self.volumes.iter().enumerate() {
            keys.extend(
                a.fs.checkpoint_keys()
                    .into_iter()
                    .map(|k| ((i as u64 + 1) << 40) | k),
            );
        }
        keys.extend(self.endpoints.iter().map(|e| ENDPOINT | e.channel.0));
        keys.extend(self.controls.iter().map(|c| ENDPOINT | c.channel.0));
        keys
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        if key == 0 {
            w.word(1);
            w.word(self.control.0);
            w.word(self.migration.map_or(0, |c| c.0));
            w.word(self.volumes.len() as u64);
        } else if key & ENDPOINT != 0 {
            if let Some(c) = self
                .controls
                .iter()
                .find(|c| c.channel.0 == key & !ENDPOINT)
            {
                w.word(c.channel.0);
                w.word(u64::MAX);
                w.word(c.volume as u64);
                return Ok(Some(w.finish()));
            }
            let Some(e) = self
                .endpoints
                .iter()
                .find(|e| e.channel.0 == key & !ENDPOINT)
            else {
                return Ok(None);
            };
            w.word(e.channel.0);
            w.word(e.volume as u64);
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
            let Some(volume) = self.volumes.get(id) else {
                return Ok(None);
            };
            let local = key & ((1 << 40) - 1);
            if local != 0 {
                return volume.fs.checkpoint_record(local);
            }
            w.bytes(&volume.fs.checkpoint_record(0)?.ok_or(Error::BadState)?);
            match &volume.device {
                VolumeDevice::Partition(partition) => {
                    w.word(0);
                    w.word(partition.first);
                    w.word(partition.sectors);
                    w.bytes(&partition.disk.checkpoint());
                }
                VolumeDevice::Raw4k(disk) => {
                    w.word(1);
                    w.word(0);
                    w.word(0);
                    w.bytes(&disk.checkpoint());
                }
            }
            w.word(volume.key_handle);
            w.word(volume.managed as u64);
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key & ENDPOINT != 0 {
            self.endpoints.retain(|e| e.channel.0 != key & !ENDPOINT);
            self.controls.retain(|c| c.channel.0 != key & !ENDPOINT);
            let Some(bytes) = bytes else {
                return Ok(());
            };
            let mut r = Decoder::new(bytes);
            let channel = Channel(r.word()?);
            if channel.0 != key & !ENDPOINT {
                return Err(Error::InvalidData);
            }
            let marker_or_volume = r.word()?;
            if marker_or_volume == u64::MAX {
                let volume = r.count(1024)?;
                r.finish()?;
                self.controls.push(MountControl { channel, volume });
                return Ok(());
            }
            let volume = usize::try_from(marker_or_volume).map_err(|_| Error::InvalidData)?;
            if volume > 1024 {
                return Err(Error::Capacity);
            }
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
                volume,
                inode,
                file,
            });
            return Ok(());
        }
        if bytes.is_none() && key != 0 {
            let id = (key >> 40).checked_sub(1).ok_or(Error::InvalidData)? as usize;
            if let Some(v) = self.volumes.get_mut(id) {
                v.fs.adopt_record(key & ((1 << 40) - 1), None)?;
            }
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
            self.expected_volumes = count;
            self.volumes.truncate(count);
        } else {
            let id = (key >> 40).checked_sub(1).ok_or(Error::InvalidData)? as usize;
            let local = key & ((1 << 40) - 1);
            if local == 0 {
                let mut r = Decoder::new(bytes);
                let metadata = r.bytes(1024)?;
                let device_kind = r.word()?;
                let first = r.word()?;
                let sectors = r.word()?;
                let disk = SharedBlock::adopt(r.bytes(1024)?)?;
                let key_handle = r.word()?;
                let managed = r.flag()?;
                r.finish()?;
                let device = match device_kind {
                    0 => {
                        if first % 8 != 0 || sectors % 8 != 0 {
                            return Err(Error::InvalidData);
                        }
                        VolumeDevice::Partition(Partition {
                            disk,
                            first,
                            sectors,
                        })
                    }
                    1 => VolumeDevice::Raw4k(disk),
                    _ => return Err(Error::InvalidData),
                };
                if id > self.volumes.len() || id >= self.expected_volumes || key_handle == 0 {
                    return Err(Error::InvalidData);
                }
                if id == self.volumes.len() {
                    self.volumes.push(Volume {
                        fs: BexFs::adopt_metadata(metadata)?,
                        device,
                        key_handle,
                        owned: false,
                        managed,
                        prepared: None,
                    });
                } else {
                    let v = &mut self.volumes[id];
                    v.fs.adopt_record(0, Some(metadata))?;
                    v.device = device;
                    v.key_handle = key_handle;
                    v.managed = managed;
                }
            } else {
                self.volumes
                    .get_mut(id)
                    .ok_or(Error::InvalidData)?
                    .fs
                    .adopt_record(local, Some(bytes))?;
            }
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        for a in &self.volumes {
            a.fs.validate_checkpoint()?;
        }
        for e in &self.endpoints {
            let fs = &self.volumes.get(e.volume).ok_or(Error::InvalidData)?.fs;
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
        for c in &self.controls {
            if c.volume >= self.volumes.len() {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out: Vec<_> = core::iter::once(self.control.0)
            .chain(self.migration.map(|c| c.0))
            .chain(self.endpoints.iter().map(|e| e.channel.0))
            .chain(self.controls.iter().map(|c| c.channel.0))
            .map(Resource::Handle)
            .collect();
        for v in &self.volumes {
            out.push(Resource::Handle(v.key_handle));
            out.extend(v.device.resources());
        }
        out
    }
    fn activated(&mut self, generation: u64) {
        for v in &mut self.volumes {
            match &v.device {
                VolumeDevice::Partition(partition) => partition.disk.activate(),
                VolumeDevice::Raw4k(disk) => disk.activate(),
            }
            v.owned = true;
            let va = Memory::map(v.key_handle, 4096, 2).expect("adopted volume key");
            let key =
                LockedVolumeKey::new(unsafe { core::slice::from_raw_parts(va as *const u8, 32) })
                    .unwrap();
            Memory::unmap(va, 4096).unwrap();
            v.fs.activate_key(key);
        }
        log(&alloc::format!(
            "bexfs: adopted generation={generation}; nodes and cursors retained\n"
        ));
    }
}
