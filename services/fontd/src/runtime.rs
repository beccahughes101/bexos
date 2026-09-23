use crate::{
    binding::Client,
    index::{Index, Query, Scope, digest},
    parser,
    resolver::{DisabledResolver, MissingFontResolver},
    storage, system,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use bexos_userspace::{Channel, Memory, Rpc};
use fonts_fidl::{FontFormat, FontHandle, FontStatus, FontStyle};
use user_manager_fidl::{
    FidlDecode, FidlEncode, UserManagerGetUserRequest, UserManagerGetUserResponse, UserStatus,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Blob {
    pub handle: u64,
    pub len: u64,
}

pub struct Runtime {
    pub remote: crate::remote::Remote,
    pub control: Channel,
    pub migration: Option<Channel>,
    pub vfsd: Channel,
    pub usersd: Channel,
    pub watcher: Channel,
    pub clients: Vec<Client>,
    pub index: Index,
    pub blobs: BTreeMap<[u8; 32], Blob>,
    pub loaded_users: BTreeSet<u64>,
}

impl Runtime {
    pub fn empty() -> Self {
        Self {
            remote: crate::remote::Remote::default(),
            control: Channel(0),
            migration: None,
            vfsd: Channel(0),
            usersd: Channel(0),
            watcher: Channel(0),
            clients: Vec::new(),
            index: Index::default(),
            blobs: BTreeMap::new(),
            loaded_users: BTreeSet::new(),
        }
    }

    pub fn add_system_fonts(&mut self) -> Result<(), FontStatus> {
        for font in system::fonts() {
            let metadata = [parser::FaceMetadata {
                family: font.family.into(),
                weight: 400,
                style: FontStyle::Normal,
                format: FontFormat::Truetype,
                index: 0,
                scripts: font.scripts,
            }];
            if !self.blobs.contains_key(&font.digest) {
                self.blobs.insert(
                    font.digest,
                    Blob {
                        handle: storage::read_only_vmo(font.bytes)?,
                        len: font.bytes.len() as u64,
                    },
                );
            }
            self.index
                .insert(Scope::System, font.digest, &metadata)
                .map_err(|_| FontStatus::ResourceExhausted)?;
        }
        Ok(())
    }

    pub fn check_user(&mut self, uid: u64) -> Result<(), FontStatus> {
        if uid == 0 || self.usersd.0 == 0 {
            return if uid == 0 {
                Ok(())
            } else {
                Err(FontStatus::AccessDenied)
            };
        }
        let mut bytes = [0; 16];
        let encoded = UserManagerGetUserRequest { uid }
            .encode(&mut bytes, &mut [])
            .map_err(|_| FontStatus::InvalidArgs)?;
        let message = Rpc(self.usersd)
            .call_raw(2, &bytes[..encoded.bytes], &[], true)
            .map_err(|_| FontStatus::Storage)?;
        let response = UserManagerGetUserResponse::decode(&message.bytes, &[])
            .map_err(|_| FontStatus::Storage)?;
        if response.status != UserStatus::Ok || !response.user.unlocked || response.user.disabled {
            self.evict_user(uid);
            return Err(FontStatus::AccessDenied);
        }
        Ok(())
    }

    pub fn resolve(
        &mut self,
        uid: u64,
        query: &Query,
        allow_network_fetch: bool,
    ) -> Result<FontHandle, FontStatus> {
        if query.weight == 0
            || query.weight > 1000
            || crate::index::normalize_family(&query.family).is_none()
        {
            return Err(FontStatus::InvalidArgs);
        }
        if uid != 0 {
            let _ = storage::load_user(self, uid);
        }
        let Some(face) = self.index.resolve(uid, query).cloned() else {
            let mut resolver = DisabledResolver;
            let _ = allow_network_fetch;
            resolver.resolve(query)?;
            return Err(FontStatus::NotFound);
        };
        self.handle_for(&face)
    }

    pub fn fallbacks(&mut self, uid: u64, script: &str) -> Result<Vec<FontHandle>, FontStatus> {
        if uid != 0 {
            let _ = storage::load_user(self, uid);
        }
        let faces = self
            .index
            .fallbacks(uid, script)
            .ok_or(FontStatus::InvalidArgs)?;
        let mut result = Vec::with_capacity(faces.len());
        for face in faces {
            match self.handle_for(face) {
                Ok(handle) => result.push(handle),
                Err(status) => {
                    for handle in result {
                        let _ = Memory::close(handle.data.raw);
                    }
                    return Err(status);
                }
            }
        }
        Ok(result)
    }

    pub fn install(&mut self, uid: u64, handle: u64, len: u64) -> Result<u64, FontStatus> {
        let result = (|| {
            if uid == 0 || len == 0 || len > parser::MAX_FONT_BYTES as u64 {
                return Err(FontStatus::InvalidArgs);
            }
            self.check_user(uid)?;
            storage::load_user(self, uid)?;
            let bytes = read_vmo(handle, len)?;
            let metadata = parser::parse(&bytes).map_err(parser_status)?;
            let digest = digest(&bytes);
            storage::persist(self, uid, &bytes, &digest)?;
            if !self.blobs.contains_key(&digest) {
                self.blobs.insert(
                    digest,
                    Blob {
                        handle: storage::read_only_vmo(&bytes)?,
                        len,
                    },
                );
            }
            let ids = self
                .index
                .insert(Scope::User(uid), digest, &metadata)
                .map_err(|_| FontStatus::ResourceExhausted)?;
            self.loaded_users.insert(uid);
            ids.first().copied().ok_or(FontStatus::InvalidArgs)
        })();
        let _ = Memory::close(handle);
        result
    }

    pub fn evict_user(&mut self, uid: u64) {
        self.index.remove_user(uid);
        self.loaded_users.remove(&uid);
        let used = self
            .index
            .faces()
            .iter()
            .map(|face| face.digest)
            .collect::<BTreeSet<_>>();
        self.blobs.retain(|digest, blob| {
            if used.contains(digest) {
                true
            } else {
                let _ = Memory::close(blob.handle);
                false
            }
        });
    }

    fn handle_for(&self, face: &crate::index::Face) -> Result<FontHandle, FontStatus> {
        let blob = self.blobs.get(&face.digest).ok_or(FontStatus::Storage)?;
        let data = Memory::duplicate(blob.handle, 1 | 2 | 16)
            .map_err(|_| FontStatus::ResourceExhausted)?;
        Ok(FontHandle {
            font_id: face.id,
            data: fonts_fidl::HandleRef { raw: data },
            data_len: blob.len,
            index_in_collection: face.metadata.index,
        })
    }
}

fn read_vmo(handle: u64, len: u64) -> Result<Vec<u8>, FontStatus> {
    let address = Memory::map(handle, len, 2).map_err(|_| FontStatus::AccessDenied)?;
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, len as usize) }.to_vec();
    let _ = Memory::unmap(address, len);
    Ok(bytes)
}

pub fn parser_status(error: parser::Error) -> FontStatus {
    match error {
        parser::Error::Unsupported => FontStatus::UnsupportedFormat,
        parser::Error::Invalid | parser::Error::Bounds => FontStatus::InvalidArgs,
    }
}
