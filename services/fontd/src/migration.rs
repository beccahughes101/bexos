use crate::{
    binding::Client,
    index::{Face, Scope},
    parser::FaceMetadata,
    runtime::{Blob, Runtime},
};
use alloc::{string::ToString, vec, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use fonts_fidl::{FontFormat, FontStyle};

const KEY_RUNTIME: u64 = 0;
const KEY_INDEX: u64 = 1;
const KEY_BLOBS: u64 = 2;

impl State for Runtime {
    fn empty() -> Self {
        Runtime::empty()
    }

    fn keys(&self) -> Vec<u64> {
        vec![KEY_RUNTIME, KEY_INDEX, KEY_BLOBS, 3]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut writer = Encoder::new();
        match key {
            3 => return Ok(Some(self.remote.encode())),
            KEY_RUNTIME => {
                writer.word(1);
                writer.word(self.control.0);
                writer.word(self.migration.map_or(0, |channel| channel.0));
                writer.word(self.vfsd.0);
                writer.word(self.usersd.0);
                writer.word(self.watcher.0);
                writer.word(self.clients.len() as u64);
                for client in &self.clients {
                    writer.word(client.channel);
                    writer.word(client.uid);
                    writer.word(client.install as u64);
                    writer.word(client.methods.len() as u64);
                    for method in &client.methods {
                        writer.word(*method);
                    }
                }
                writer.word(self.loaded_users.len() as u64);
                for uid in &self.loaded_users {
                    writer.word(*uid);
                }
            }
            KEY_INDEX => {
                writer.word(1);
                writer.word(self.index.faces().len() as u64);
                for face in self.index.faces() {
                    writer.word(face.id);
                    writer.bytes(&face.digest);
                    match face.scope {
                        Scope::System => {
                            writer.word(0);
                            writer.word(0);
                        }
                        Scope::User(uid) => {
                            writer.word(1);
                            writer.word(uid);
                        }
                    }
                    writer.text(&face.metadata.family);
                    writer.word(face.metadata.weight as u64);
                    writer.word(face.metadata.style as u64);
                    writer.word(face.metadata.format as u64);
                    writer.word(face.metadata.index as u64);
                    writer.word(face.metadata.scripts as u64);
                }
            }
            KEY_BLOBS => {
                writer.word(1);
                writer.word(self.blobs.len() as u64);
                for (digest, blob) in &self.blobs {
                    writer.bytes(digest);
                    writer.word(blob.handle);
                    writer.word(blob.len);
                }
            }
            _ => return Ok(None),
        }
        Ok(Some(writer.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 3 {
            self.remote = crate::remote::Remote::decode(bytes.ok_or(Error::InvalidData)?)?;
            return Ok(());
        }
        let mut reader = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if reader.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        match key {
            KEY_RUNTIME => {
                self.control = Channel(reader.word()?);
                let migration = reader.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.vfsd = Channel(reader.word()?);
                self.usersd = Channel(reader.word()?);
                self.watcher = Channel(reader.word()?);
                self.clients.clear();
                for _ in 0..reader.count(512)? {
                    let channel = reader.word()?;
                    let uid = reader.word()?;
                    let install = reader.flag()?;
                    let mut methods = Vec::new();
                    for _ in 0..reader.count(3)? {
                        methods.push(reader.word()?);
                    }
                    self.clients.push(Client {
                        channel,
                        uid,
                        methods,
                        install,
                    });
                }
                self.loaded_users.clear();
                for _ in 0..reader.count(256)? {
                    self.loaded_users.insert(reader.word()?);
                }
            }
            KEY_INDEX => {
                let mut faces = Vec::new();
                for _ in 0..reader.count(4096)? {
                    let id = reader.word()?;
                    let digest: [u8; 32] = reader
                        .bytes(32)?
                        .try_into()
                        .map_err(|_| Error::InvalidData)?;
                    let scope = match reader.word()? {
                        0 => {
                            let _ = reader.word()?;
                            Scope::System
                        }
                        1 => Scope::User(reader.word()?),
                        _ => return Err(Error::InvalidData),
                    };
                    let family = reader.text(64)?.to_string();
                    let weight = u16::try_from(reader.word()?).map_err(|_| Error::InvalidData)?;
                    let style = decode_style(reader.word()?)?;
                    let format = decode_format(reader.word()?)?;
                    let index = u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?;
                    let scripts = u8::try_from(reader.word()?).map_err(|_| Error::InvalidData)?;
                    faces.push(Face {
                        id,
                        digest,
                        scope,
                        metadata: FaceMetadata {
                            family,
                            weight,
                            style,
                            format,
                            index,
                            scripts,
                        },
                    });
                }
                self.index.replace(faces);
            }
            KEY_BLOBS => {
                self.blobs.clear();
                for _ in 0..reader.count(4096)? {
                    let digest: [u8; 32] = reader
                        .bytes(32)?
                        .try_into()
                        .map_err(|_| Error::InvalidData)?;
                    let handle = reader.word()?;
                    let len = reader.word()?;
                    self.blobs.insert(digest, Blob { handle, len });
                }
            }
            _ => return Ok(()),
        }
        reader.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.vfsd.0 == 0 || self.usersd.0 == 0
        {
            return Err(Error::InvalidData);
        }
        if self
            .index
            .faces()
            .iter()
            .any(|face| !self.blobs.contains_key(&face.digest))
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = vec![self.control.0, self.vfsd.0, self.usersd.0];
        handles.push(self.remote.endpoint);
        handles.push(self.remote.directory);
        if let Some(channel) = self.migration {
            handles.push(channel.0);
        }
        if self.watcher.0 != 0 {
            handles.push(self.watcher.0);
        }
        handles.extend(self.clients.iter().map(|client| client.channel));
        handles.extend(self.blobs.values().map(|blob| blob.handle));
        handles
            .into_iter()
            .filter(|handle| *handle != 0)
            .map(Resource::Handle)
            .collect()
    }

    fn activated(&mut self, _generation: u64) {}
}

fn decode_style(value: u64) -> Result<FontStyle, Error> {
    match value {
        1 => Ok(FontStyle::Normal),
        2 => Ok(FontStyle::Italic),
        3 => Ok(FontStyle::Oblique),
        _ => Err(Error::InvalidData),
    }
}

fn decode_format(value: u64) -> Result<FontFormat, Error> {
    match value {
        1 => Ok(FontFormat::Truetype),
        2 => Ok(FontFormat::Opentype),
        3 => Ok(FontFormat::Woff2),
        4 => Ok(FontFormat::Collection),
        _ => Err(Error::InvalidData),
    }
}
