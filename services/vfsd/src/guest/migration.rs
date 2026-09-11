use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub store: Option<PackageStore>,
    pub tmp: Option<TmpManager>,
    pub clients: Vec<bexos_userspace::service_binding::BoundServiceEndpoint>,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            store: None,
            tmp: None,
            clients: Vec::new(),
        }
    }
    fn keys(&self) -> Vec<u64> {
        vec![0]
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        w.word(4);
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.store.is_some() as u64);
        if let Some(s) = &self.store {
            for h in [s.archivefs, s.bexfs, s.diskimage, s.storage_root, s.root] {
                w.word(h.0);
            }
            w.word(s.users.len() as u64);
            for user in &s.users {
                w.word(user.uid);
                w.word(user.root.0);
                w.word(user.control.0);
                w.word(user.ukek_vmo);
                w.word(user.active_slot as u64);
                w.word(user.virtual_size_bytes);
            }
        }
        w.word(self.tmp.is_some() as u64);
        if let Some(tmp) = &self.tmp {
            w.word(tmp.memfs.0);
        }
        w.word(self.clients.len() as u64);
        for client in &self.clients {
            w.word(client.channel.0);
            w.word(client.allowed_methods.len() as u64);
            for ordinal in &client.allowed_methods {
                w.word(*ordinal);
            }
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if version != 1 && version != 2 && version != 3 && version != 4 {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(r.word()?);
        let h = r.word()?;
        self.migration = (h != 0).then_some(Channel(h));
        self.store = if r.flag()? {
            let archivefs = Channel(r.word()?);
            let (bexfs, diskimage, storage_root, root, users) = if version >= 3 {
                let bexfs = Channel(r.word()?);
                let diskimage = Channel(r.word()?);
                let storage_root = Channel(r.word()?);
                let root = Channel(r.word()?);
                let mut users = Vec::new();
                for _ in 0..r.count(1024)? {
                    users.push(UserMount {
                        uid: r.word()?,
                        root: Channel(r.word()?),
                        control: Channel(r.word()?),
                        ukek_vmo: r.word()?,
                        active_slot: u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                        virtual_size_bytes: r.word()?,
                    });
                }
                (bexfs, diskimage, storage_root, root, users)
            } else {
                let storage_root = Channel(r.word()?);
                let root = Channel(r.word()?);
                (Channel(0), Channel(0), storage_root, root, Vec::new())
            };
            Some(PackageStore {
                archivefs,
                bexfs,
                diskimage,
                storage_root,
                root,
                users,
            })
        } else {
            None
        };
        self.tmp = if version >= 2 && r.flag()? {
            Some(TmpManager {
                memfs: Channel(r.word()?),
            })
        } else {
            None
        };
        self.clients.clear();
        if version >= 4 {
            for _ in 0..r.count(1024)? {
                let channel = Channel(r.word()?);
                let mut method_ordinals = Vec::new();
                for _ in 0..r.count(128)? {
                    method_ordinals.push(r.word()?);
                }
                self.clients
                    .push(bexos_userspace::service_binding::BoundServiceEndpoint::new(
                        channel,
                        method_ordinals,
                    ));
            }
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.store.as_ref().is_some_and(|s| {
                s.archivefs.0 == 0
                    || s.storage_root.0 == 0
                    || s.root.0 == 0
                    || s.bexfs.0 == 0
                    || s.diskimage.0 == 0
                    || s.users
                        .iter()
                        .any(|u| u.uid == 0 || u.root.0 == 0 || u.control.0 == 0 || u.ukek_vmo == 0)
            })
            || self.tmp.as_ref().is_some_and(|tmp| tmp.memfs.0 == 0)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut h = vec![self.control.0];
        if let Some(c) = self.migration {
            h.push(c.0);
        }
        if let Some(s) = &self.store {
            h.extend([
                s.archivefs.0,
                s.bexfs.0,
                s.diskimage.0,
                s.storage_root.0,
                s.root.0,
            ]);
            for user in &s.users {
                h.extend([user.root.0, user.control.0, user.ukek_vmo]);
            }
        }
        if let Some(tmp) = &self.tmp {
            h.push(tmp.memfs.0);
        }
        h.extend(self.clients.iter().map(|client| client.channel.0));
        h.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, generation: u64) {
        log(&alloc::format!(
            "vfsd: adopted generation={generation}; mounts retained\n"
        ));
    }
}
