use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

impl<H: HardwareKeyProvider> KeychainService<H> {
    pub(crate) fn checkpoint_stores(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        encode_store(&mut w, &self.system);
        w.word(self.users.len() as u64);
        for user in &self.users {
            w.word(user.uid);
            encode_store(&mut w, &user.store);
        }
        w.finish()
    }

    pub(crate) fn validate_stores(&self, bytes: &[u8]) -> Result<(), Error> {
        let (system, users) = decode_stores(bytes)?;
        #[cfg(feature = "persistent")]
        if self.vfsd.is_none()
            && (matches!(system, VaultStore::Reopen)
                || users
                    .iter()
                    .any(|user| matches!(user.store, VaultStore::Reopen)))
        {
            return Err(Error::InvalidData);
        }
        let _ = (system, users);
        Ok(())
    }

    pub(crate) fn adopt_stores(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.validate_stores(bytes)?;
        (self.system, self.users) = decode_stores(bytes)?;
        Ok(())
    }
}

fn encode_store(w: &mut Encoder, store: &VaultStore) {
    match store {
        VaultStore::Memory(store) => {
            w.word(0);
            w.bytes(&store.checkpoint());
        }
        #[cfg(feature = "persistent")]
        VaultStore::Persistent(_) | VaultStore::Reopen => {
            w.word(1);
            w.bytes(&[]);
        }
    }
}
fn decode_store(r: &mut Decoder<'_>) -> Result<VaultStore, Error> {
    let kind = r.word()?;
    let bytes = r.bytes(8 * 1024 * 1024)?;
    match kind {
        0 => Ok(VaultStore::Memory(
            MemoryKeychainStore::from_checkpoint(bytes).map_err(|_| Error::InvalidData)?,
        )),
        #[cfg(feature = "persistent")]
        1 if bytes.is_empty() => Ok(VaultStore::Reopen),
        _ => Err(Error::InvalidData),
    }
}
fn decode_stores(bytes: &[u8]) -> Result<(VaultStore, Vec<UserVault>), Error> {
    let mut r = Decoder::new(bytes);
    if r.word()? != 1 {
        return Err(Error::UnsupportedVersion);
    }
    let system = decode_store(&mut r)?;
    let mut users: Vec<UserVault> = Vec::new();
    for _ in 0..r.count(4096)? {
        let uid = r.word()?;
        if uid == 0 || users.iter().any(|user| user.uid == uid) {
            return Err(Error::InvalidData);
        }
        users.push(UserVault {
            uid,
            store: decode_store(&mut r)?,
        });
    }
    r.finish()?;
    Ok((system, users))
}

#[cfg(feature = "persistent")]
pub(super) fn reopen(
    store: &mut VaultStore,
    vfsd: Option<Channel>,
    uid: Option<u64>,
) -> Result<(), KeychainStatus> {
    if matches!(store, VaultStore::Reopen) {
        let vfsd = vfsd.ok_or(KeychainStatus::ErrStorage)?;
        let database = match uid {
            Some(uid) => open_user_keychain(vfsd, uid)?,
            None => open_system_keychain(vfsd)?,
        };
        *store = VaultStore::Persistent(database);
    }
    Ok(())
}

#[cfg(feature = "persistent")]
impl<H: HardwareKeyProvider> KeychainService<H> {
    pub(crate) fn adopt_legacy_stores(&mut self, uids: &[u64]) -> Result<(), Error> {
        if self.vfsd.is_none() {
            return Err(Error::InvalidData);
        }
        self.system = VaultStore::Reopen;
        self.users = uids
            .iter()
            .map(|uid| UserVault {
                uid: *uid,
                store: VaultStore::Reopen,
            })
            .collect();
        Ok(())
    }
}
