use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};

pub const ATTACHMENT: u64 = 1 << 63;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub attachments: Vec<Attachment>,
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            attachments: Vec::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        core::iter::once(0)
            .chain(
                self.attachments
                    .iter()
                    .map(|attachment| ATTACHMENT | attachment.control.0),
            )
            .collect()
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut encoder = Encoder::new();
        if key == 0 {
            encoder.word(1);
            encoder.word(self.control.0);
            encoder.word(self.migration.map_or(0, |channel| channel.0));
            encoder.word(self.attachments.len() as u64);
            return Ok(Some(encoder.finish()));
        }
        let Some(attachment) = self
            .attachments
            .iter()
            .find(|attachment| attachment.control.0 == key & !ATTACHMENT)
        else {
            return Ok(None);
        };
        let info = attachment.server.info();
        encoder.word(attachment.control.0);
        encoder.word(attachment.server.file_handle().0);
        encoder.word(attachment.server.encrypted_info().is_some() as u64);
        encoder.word(attachment.key_handle);
        encoder.word(u64::from(info.flags.0));
        encoder.word(attachment.server.next_vmo_id().into());
        encoder.word(attachment.buffers.len() as u64);
        for (id, (handle, va, blocks)) in &attachment.buffers {
            encoder.word(u64::from(*id));
            encoder.word(*handle);
            encoder.word(*va);
            encoder.word(*blocks);
        }
        encoder.word(attachment.fifos.len() as u64);
        for fifo in &attachment.fifos {
            encoder.word(fifo.0);
        }
        Ok(Some(encoder.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 0 {
            let mut decoder = Decoder::new(bytes.ok_or(Error::InvalidData)?);
            if decoder.word()? != 1 {
                return Err(Error::UnsupportedVersion);
            }
            self.control = Channel(decoder.word()?);
            self.migration = Some(Channel(decoder.word()?));
            let count = decoder.count(1024)?;
            decoder.finish()?;
            self.attachments.truncate(count);
            return Ok(());
        }
        self.attachments
            .retain(|attachment| attachment.control.0 != key & !ATTACHMENT);
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let mut decoder = Decoder::new(bytes);
        let control = Channel(decoder.word()?);
        if control.0 != key & !ATTACHMENT {
            return Err(Error::InvalidData);
        }
        let file = FsBackingFile {
            file: Channel(decoder.word()?),
            owned: false,
        };
        let encrypted = decoder.flag()?;
        let key_handle = decoder.word()?;
        let read_only = decoder.word()? & u64::from(BlockFlags::READ_ONLY.0) != 0;
        let next_vmo_id = u32::try_from(decoder.word()?).map_err(|_| Error::InvalidData)?;
        let mut key_bytes = [0u8; 32];
        let mut server = if encrypted {
            if key_handle == 0 {
                return Err(Error::InvalidData);
            }
            let va = Memory::map(key_handle, 4096, 2).map_err(|_| Error::InvalidData)?;
            key_bytes.copy_from_slice(unsafe { core::slice::from_raw_parts(va as *const u8, 32) });
            let server = DiskImageServer::open_encrypted(file, &key_bytes, read_only)
                .map_err(|_| Error::InvalidData)?;
            key_bytes.fill(0);
            Memory::unmap(va, 4096).map_err(|_| Error::InvalidData)?;
            server
        } else {
            if key_handle != 0 {
                return Err(Error::InvalidData);
            }
            DiskImageServer::attach(file, read_only).map_err(|_| Error::InvalidData)?
        };
        let mut buffers = BTreeMap::new();
        let mut registered = BTreeMap::new();
        for _ in 0..decoder.count(1024)? {
            let id = u32::try_from(decoder.word()?).map_err(|_| Error::InvalidData)?;
            let handle = decoder.word()?;
            let va = decoder.word()?;
            let blocks = decoder.word()?;
            if buffers.insert(id, (handle, va, blocks)).is_some()
                || registered
                    .insert(id, RegisteredBuffer { handle, blocks })
                    .is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        server.restore_registrations(next_vmo_id, registered);
        let mut fifos = Vec::new();
        for _ in 0..decoder.count(1024)? {
            fifos.push(Channel(decoder.word()?));
        }
        decoder.finish()?;
        self.attachments.push(Attachment {
            control,
            server,
            buffers,
            fifos,
            key_handle,
        });
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        if self
            .attachments
            .iter()
            .any(|attachment| !attachment.server.registrations_match(&attachment.buffers))
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out: Vec<_> = core::iter::once(self.control.0)
            .chain(self.migration.map(|channel| channel.0))
            .map(Resource::Handle)
            .collect();
        for attachment in &self.attachments {
            out.push(Resource::Handle(attachment.control.0));
            out.push(Resource::Handle(attachment.server.file_handle().0));
            if attachment.key_handle != 0 {
                out.push(Resource::Handle(attachment.key_handle));
            }
            out.extend(attachment.fifos.iter().map(|fifo| Resource::Handle(fifo.0)));
            for (handle, va, blocks) in attachment.buffers.values() {
                out.push(Resource::Mapping {
                    handle: *handle,
                    offset: 0,
                    va: *va,
                    size: u64::from(DISKIMAGE_BLOCK_SIZE) * *blocks,
                    rights: 6,
                });
            }
        }
        out
    }

    fn activated(&mut self, generation: u64) {
        for attachment in &mut self.attachments {
            attachment.server.file_mut().owned = true;
        }
        log(&alloc::format!(
            "diskimage: adopted generation={generation}; attachments retained\n"
        ));
    }
}
