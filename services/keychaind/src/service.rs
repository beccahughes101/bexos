mod snapshot;
use alloc::vec;
use alloc::vec::Vec;
use bexos_crypto::AES_GCM_NONCE_LEN;
use bexos_keychain_store::SecretRecord;
#[cfg(feature = "persistent")]
use bexos_keychain_store::persistent::KeychainStoreDb;
use bexos_keychain_store::{
    KeyAlgorithm as StoreKeyAlgorithm, KeyFlags as StoreKeyFlags, KeychainStoreError,
    MemoryKeychainStore,
};
#[cfg(feature = "persistent")]
use bexos_redb::bexos_fs::{FileBlockStore, defer_file_syncs};
use bexos_trusty_client::protocol::{HW_AUTH_TOKEN_TIMEOUT_SECS, KEYMINT_UUID, TrustyWireError};
use bexos_trusty_client::services::{
    GeneratedKey, KEYMINT_CMD_BEGIN, KEYMINT_CMD_DELETE_KEY, KEYMINT_CMD_FINISH,
    KEYMINT_CMD_GENERATE_KEY, KEYMINT_CMD_UPDATE_AAD, KeyMintAlgorithm, KeyMintPurpose,
    decode_keymint_begin, decode_keymint_delete_key, decode_keymint_finish,
    decode_keymint_generated_key, decode_keymint_signature, decode_keymint_update_aad,
    encode_keymint_begin, encode_keymint_delete_key, encode_keymint_finish,
    encode_keymint_generate_key, encode_keymint_update_aad,
};
use bexos_userspace::{Channel, Memory, Rpc};
#[cfg(feature = "persistent")]
use bexos_userspace::{fs, vfs};
use keychain_fidl::{KeyAlgorithm, KeyFlags, KeychainScope, KeychainStatus};
use tee_manager_fidl as tee;
use tee_manager_fidl::{FidlDecode as TeeFidlDecode, FidlEncode as TeeFidlEncode};

#[cfg(feature = "persistent")]
const KEYCHAIN_DB_FILE: &str = "keychain.redb";
#[cfg(feature = "persistent")]
pub(crate) const KEYCHAIND_PACKAGE: &str = "bexos.service.keychaind";

pub struct KeychainService<H = UnsupportedHardwareKeyProvider> {
    system: VaultStore,
    users: Vec<UserVault>,
    pub(crate) hardware: H,
    #[cfg(feature = "persistent")]
    pub(crate) vfsd: Option<Channel>,
}

struct UserVault {
    uid: u64,
    store: VaultStore,
}

enum VaultStore {
    Memory(MemoryKeychainStore),
    #[cfg(feature = "persistent")]
    Persistent(KeychainStoreDb),
    #[cfg(feature = "persistent")]
    Reopen,
}

pub trait HardwareKeyProvider {
    fn generate_key(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<GeneratedKey, KeychainStoreError>;
    fn sign(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        algorithm: KeyAlgorithm,
        opaque_key_blob: &[u8],
        digest: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError>;
    fn aead_encrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        plaintext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<([u8; AES_GCM_NONCE_LEN], Vec<u8>), KeychainStoreError>;
    fn aead_decrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError>;
    fn delete_key(&mut self, opaque_key_blob: &[u8]) -> Result<(), KeychainStoreError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnsupportedHardwareKeyProvider;

impl HardwareKeyProvider for UnsupportedHardwareKeyProvider {
    fn generate_key(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        _algorithm: KeyAlgorithm,
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<GeneratedKey, KeychainStoreError> {
        Err(KeychainStoreError::Unsupported)
    }

    fn sign(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        _algorithm: KeyAlgorithm,
        _opaque_key_blob: &[u8],
        _digest: &[u8],
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        Err(KeychainStoreError::Unsupported)
    }

    fn aead_encrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        _opaque_key_blob: &[u8],
        _plaintext: &[u8],
        _aad: &[u8],
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<([u8; AES_GCM_NONCE_LEN], Vec<u8>), KeychainStoreError> {
        Err(KeychainStoreError::Unsupported)
    }

    fn aead_decrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        _opaque_key_blob: &[u8],
        _nonce: &[u8],
        _ciphertext: &[u8],
        _aad: &[u8],
        _hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        Err(KeychainStoreError::Unsupported)
    }

    fn delete_key(&mut self, _opaque_key_blob: &[u8]) -> Result<(), KeychainStoreError> {
        Err(KeychainStoreError::Unsupported)
    }
}

#[derive(Clone, Copy)]
pub struct TeeKeyMintClient {
    client: Channel,
    session_id: Option<u64>,
}

#[derive(Clone, Copy)]
pub enum RuntimeHardwareKeyProvider {
    Unsupported(UnsupportedHardwareKeyProvider),
    Tee(TeeKeyMintClient),
}

impl HardwareKeyProvider for RuntimeHardwareKeyProvider {
    fn generate_key(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<GeneratedKey, KeychainStoreError> {
        match self {
            Self::Unsupported(provider) => {
                provider.generate_key(scope, uid, alias, algorithm, hardware_auth_token)
            }
            Self::Tee(provider) => {
                provider.generate_key(scope, uid, alias, algorithm, hardware_auth_token)
            }
        }
    }

    fn sign(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        opaque_key_blob: &[u8],
        digest: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        match self {
            Self::Unsupported(provider) => provider.sign(
                scope,
                uid,
                alias,
                algorithm,
                opaque_key_blob,
                digest,
                hardware_auth_token,
            ),
            Self::Tee(provider) => provider.sign(
                scope,
                uid,
                alias,
                algorithm,
                opaque_key_blob,
                digest,
                hardware_auth_token,
            ),
        }
    }

    fn aead_encrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        plaintext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<([u8; AES_GCM_NONCE_LEN], Vec<u8>), KeychainStoreError> {
        match self {
            Self::Unsupported(provider) => provider.aead_encrypt(
                scope,
                uid,
                alias,
                opaque_key_blob,
                plaintext,
                aad,
                hardware_auth_token,
            ),
            Self::Tee(provider) => provider.aead_encrypt(
                scope,
                uid,
                alias,
                opaque_key_blob,
                plaintext,
                aad,
                hardware_auth_token,
            ),
        }
    }

    fn aead_decrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        opaque_key_blob: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        match self {
            Self::Unsupported(provider) => provider.aead_decrypt(
                scope,
                uid,
                alias,
                opaque_key_blob,
                nonce,
                ciphertext,
                aad,
                hardware_auth_token,
            ),
            Self::Tee(provider) => provider.aead_decrypt(
                scope,
                uid,
                alias,
                opaque_key_blob,
                nonce,
                ciphertext,
                aad,
                hardware_auth_token,
            ),
        }
    }

    fn delete_key(&mut self, opaque_key_blob: &[u8]) -> Result<(), KeychainStoreError> {
        match self {
            Self::Unsupported(provider) => provider.delete_key(opaque_key_blob),
            Self::Tee(provider) => provider.delete_key(opaque_key_blob),
        }
    }
}

impl TeeKeyMintClient {
    pub fn connect(client: Channel) -> Result<Self, KeychainStoreError> {
        // The startup grant is already a restricted TeeManager connection.
        if client.0 == 0 {
            return Err(KeychainStoreError::Storage);
        }
        Ok(Self {
            client,
            session_id: None,
        })
    }

    pub(crate) fn from_parts(client: Channel, session_id: Option<u64>) -> Self {
        Self { client, session_id }
    }

    pub(crate) fn client(&self) -> Channel {
        self.client
    }

    pub(crate) fn cached_session_id(&self) -> Option<u64> {
        self.session_id
    }

    fn session_id(&mut self) -> Result<u64, KeychainStoreError> {
        if let Some(session_id) = self.session_id {
            return Ok(session_id);
        }
        let session_id = tee_open_keymint_session(self.client)?;
        self.session_id = Some(session_id);
        Ok(session_id)
    }
}

impl HardwareKeyProvider for TeeKeyMintClient {
    fn generate_key(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<GeneratedKey, KeychainStoreError> {
        let payload = encode_keymint_generate_key(
            scope_uid(scope, uid),
            alias,
            keymint_algorithm(algorithm),
            hardware_auth_token.is_some(),
            HW_AUTH_TOKEN_TIMEOUT_SECS,
            hardware_auth_token,
        )
        .map_err(|_| KeychainStoreError::InvalidAlias)?;
        let session_id = self.session_id()?;
        let response = tee_invoke(self.client, session_id, KEYMINT_CMD_GENERATE_KEY, &payload)?;
        decode_keymint_generated_key(&response).map_err(|_| KeychainStoreError::CorruptRecord)
    }

    fn sign(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        algorithm: KeyAlgorithm,
        opaque_key_blob: &[u8],
        digest: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        let keymint_algorithm = match algorithm {
            KeyAlgorithm::Aes256Gcm => return Err(KeychainStoreError::Unsupported),
            _ => keymint_algorithm(algorithm),
        };
        let session_id = self.session_id()?;
        let begin = encode_keymint_begin(
            keymint_algorithm,
            KeyMintPurpose::Sign,
            opaque_key_blob,
            None,
            hardware_auth_token,
        )
        .map_err(map_keymint_wire_error)?;
        let begun = decode_keymint_begin(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_BEGIN,
            &begin,
        )?)
        .map_err(map_keymint_wire_error)?;
        let finish = encode_keymint_finish(begun.handle, digest, hardware_auth_token)
            .map_err(map_keymint_wire_error)?;
        decode_keymint_signature(
            &tee_invoke(self.client, session_id, KEYMINT_CMD_FINISH, &finish)?,
            keymint_algorithm,
        )
        .map_err(map_keymint_wire_error)
    }

    fn aead_encrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        opaque_key_blob: &[u8],
        plaintext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<([u8; AES_GCM_NONCE_LEN], Vec<u8>), KeychainStoreError> {
        let begin = encode_keymint_begin(
            KeyMintAlgorithm::AesGcm,
            KeyMintPurpose::Encrypt,
            opaque_key_blob,
            None,
            hardware_auth_token,
        )
        .map_err(map_keymint_wire_error)?;
        let session_id = self.session_id()?;
        let begun = decode_keymint_begin(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_BEGIN,
            &begin,
        )?)
        .map_err(map_keymint_wire_error)?;
        let nonce = begun.nonce.ok_or(KeychainStoreError::CorruptRecord)?;
        if !aad.is_empty() {
            let update = encode_keymint_update_aad(begun.handle, aad, hardware_auth_token)
                .map_err(map_keymint_wire_error)?;
            decode_keymint_update_aad(&tee_invoke(
                self.client,
                session_id,
                KEYMINT_CMD_UPDATE_AAD,
                &update,
            )?)
            .map_err(map_keymint_wire_error)?;
        }
        let finish = encode_keymint_finish(begun.handle, plaintext, hardware_auth_token)
            .map_err(map_keymint_wire_error)?;
        let ciphertext = decode_keymint_finish(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_FINISH,
            &finish,
        )?)
        .map_err(map_keymint_wire_error)?;
        Ok((nonce, ciphertext))
    }

    fn aead_decrypt(
        &mut self,
        _scope: KeychainScope,
        _uid: u64,
        _alias: &str,
        opaque_key_blob: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
        hardware_auth_token: Option<&[u8]>,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        let begin = encode_keymint_begin(
            KeyMintAlgorithm::AesGcm,
            KeyMintPurpose::Decrypt,
            opaque_key_blob,
            Some(nonce),
            hardware_auth_token,
        )
        .map_err(map_keymint_wire_error)?;
        let session_id = self.session_id()?;
        let begun = decode_keymint_begin(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_BEGIN,
            &begin,
        )?)
        .map_err(map_keymint_wire_error)?;
        if !aad.is_empty() {
            let update = encode_keymint_update_aad(begun.handle, aad, hardware_auth_token)
                .map_err(map_keymint_wire_error)?;
            decode_keymint_update_aad(&tee_invoke(
                self.client,
                session_id,
                KEYMINT_CMD_UPDATE_AAD,
                &update,
            )?)
            .map_err(map_keymint_wire_error)?;
        }
        let finish = encode_keymint_finish(begun.handle, ciphertext, hardware_auth_token)
            .map_err(map_keymint_wire_error)?;
        decode_keymint_finish(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_FINISH,
            &finish,
        )?)
        .map_err(map_keymint_wire_error)
    }

    fn delete_key(&mut self, opaque_key_blob: &[u8]) -> Result<(), KeychainStoreError> {
        let request = encode_keymint_delete_key(opaque_key_blob).map_err(map_keymint_wire_error)?;
        let session_id = self.session_id()?;
        decode_keymint_delete_key(&tee_invoke(
            self.client,
            session_id,
            KEYMINT_CMD_DELETE_KEY,
            &request,
        )?)
        .map_err(map_keymint_wire_error)
    }
}

fn map_keymint_wire_error(error: TrustyWireError) -> KeychainStoreError {
    match error {
        // KM_ERROR_KEY_USER_NOT_AUTHENTICATED and verification/auth failures
        // are surfaced as access denied; callers translate a missing token to
        // ERR_LOCKED before issuing the secure operation.
        TrustyWireError::SecureService(_) => KeychainStoreError::AccessDenied,
        TrustyWireError::InvalidArgs => KeychainStoreError::InvalidSecret,
        TrustyWireError::InvalidResponse => KeychainStoreError::CorruptRecord,
    }
}

impl KeychainService<UnsupportedHardwareKeyProvider> {
    pub fn new() -> Self {
        Self {
            system: VaultStore::Memory(MemoryKeychainStore::new()),
            users: Vec::new(),
            hardware: UnsupportedHardwareKeyProvider,
            #[cfg(feature = "persistent")]
            vfsd: None,
        }
    }
}

impl<H: HardwareKeyProvider> KeychainService<H> {
    pub fn with_hardware(hardware: H) -> Self {
        Self {
            system: VaultStore::Memory(MemoryKeychainStore::new()),
            users: Vec::new(),
            hardware,
            #[cfg(feature = "persistent")]
            vfsd: None,
        }
    }

    #[cfg(feature = "persistent")]
    pub fn persistent(system: KeychainStoreDb, vfsd: Channel, hardware: H) -> Self {
        Self {
            system: VaultStore::Persistent(system),
            users: Vec::new(),
            hardware,
            vfsd: Some(vfsd),
        }
    }

    pub fn uses_volatile_storage(&self) -> bool {
        matches!(self.system, VaultStore::Memory(_))
            || self
                .users
                .iter()
                .any(|vault| matches!(vault.store, VaultStore::Memory(_)))
    }

    pub fn prepare_stop(&mut self) -> bool {
        let _ = self.checkpoint_stores();
        true
    }

    pub fn store_secret(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        secret: &[u8],
        flags: KeyFlags,
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> KeychainStatus {
        if let Err(status) = self.validate_scope(scope, uid, user_unlocked) {
            return status;
        }
        let store_flags = StoreKeyFlags(flags.0);
        if store_flags.contains(StoreKeyFlags::HARDWARE_BACKED) {
            if store_flags.contains(StoreKeyFlags::REQUIRE_USER_AUTH)
                && hardware_auth_token.is_none()
            {
                return KeychainStatus::ErrLocked;
            }
            if bexos_keychain_store::validate_alias(alias).is_err()
                || bexos_keychain_store::validate_secret(secret).is_err()
            {
                return KeychainStatus::ErrInvalidArgs;
            }
            let generated = match self.hardware.generate_key(
                scope,
                uid,
                alias,
                KeyAlgorithm::Aes256Gcm,
                hardware_auth_token,
            ) {
                Ok(generated) => generated,
                Err(error) => return status(Some(error)),
            };
            let aad = secret_envelope_aad(scope, uid, alias);
            let (nonce, ciphertext) = match self.hardware.aead_encrypt(
                scope,
                uid,
                alias,
                &generated.opaque_blob,
                secret,
                &aad,
                hardware_auth_token,
            ) {
                Ok(encrypted) => encrypted,
                Err(error) => {
                    let _ = self.hardware.delete_key(&generated.opaque_blob);
                    return status(Some(error));
                }
            };
            let new_blob = generated.opaque_blob.clone();
            let record = SecretRecord {
                alias: alias.into(),
                secret: ciphertext,
                flags: store_flags,
                generation: 0,
                envelope_key_blob: generated.opaque_blob,
                nonce: nonce.to_vec(),
                characteristics: generated.characteristics,
            };
            let old_blob = self
                .store_for_scope_mut(scope, uid, user_unlocked)
                .ok()
                .and_then(|store| store.secret_record(alias).ok())
                .filter(|old| old.flags.contains(StoreKeyFlags::HARDWARE_BACKED))
                .map(|old| old.envelope_key_blob);
            let write_result = self
                .store_for_scope_mut(scope, uid, user_unlocked)
                .and_then(|store| store.put_secret_record(record).map_err(|e| status(Some(e))));
            if let Err(write_status) = write_result {
                let _ = self.hardware.delete_key(&new_blob);
                return write_status;
            }
            if let Some(old_blob) = old_blob {
                let _ = self.hardware.delete_key(&old_blob);
            }
            return KeychainStatus::Ok;
        }
        let store = match self.store_for_scope_mut(scope, uid, user_unlocked) {
            Ok(store) => store,
            Err(status) => return status,
        };
        status(store.store_secret(alias, secret, store_flags).err())
    }

    pub fn get_secret(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> (KeychainStatus, Vec<u8>) {
        let record = match self.store_for_scope_mut(scope, uid, user_unlocked) {
            Ok(store) => match store.secret_record(alias) {
                Ok(record) => record,
                Err(error) => return (status(Some(error)), Vec::new()),
            },
            Err(status) => return (status, Vec::new()),
        };
        if record.flags.contains(StoreKeyFlags::HARDWARE_BACKED) {
            if record.flags.contains(StoreKeyFlags::REQUIRE_USER_AUTH)
                && hardware_auth_token.is_none()
            {
                return (KeychainStatus::ErrLocked, Vec::new());
            }
            let aad = secret_envelope_aad(scope, uid, alias);
            return match self.hardware.aead_decrypt(
                scope,
                uid,
                alias,
                &record.envelope_key_blob,
                &record.nonce,
                &record.secret,
                &aad,
                hardware_auth_token,
            ) {
                Ok(secret) => (KeychainStatus::Ok, secret),
                Err(error) => (status(Some(error)), Vec::new()),
            };
        }
        (KeychainStatus::Ok, record.secret)
    }

    pub fn delete_secret(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        user_unlocked: bool,
    ) -> KeychainStatus {
        let existing = match self.store_for_scope_mut(scope, uid, user_unlocked) {
            Ok(store) => match store.secret_record(alias) {
                Ok(record) => record,
                Err(error) => return status(Some(error)),
            },
            Err(status) => return status,
        };
        if existing.flags.contains(StoreKeyFlags::HARDWARE_BACKED) {
            if let Err(error) = self.hardware.delete_key(&existing.envelope_key_blob) {
                return status(Some(error));
            }
        }
        let store = match self.store_for_scope_mut(scope, uid, user_unlocked) {
            Ok(store) => store,
            Err(status) => return status,
        };
        status(store.delete_secret(alias).err())
    }

    pub fn generate_key(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        algorithm: KeyAlgorithm,
        flags: KeyFlags,
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> (KeychainStatus, Vec<u8>) {
        if keychain_flags_contain(flags, 0x0004) {
            if let Err(status) = self.validate_scope(scope, uid, user_unlocked) {
                return (status, Vec::new());
            }
            if keychain_flags_contain(flags, 0x0001) && hardware_auth_token.is_none() {
                return (KeychainStatus::ErrLocked, Vec::new());
            }
            let generated =
                match self
                    .hardware
                    .generate_key(scope, uid, alias, algorithm, hardware_auth_token)
                {
                    Ok(generated) => generated,
                    Err(error) => return (status(Some(error)), Vec::new()),
                };
            let public_key = generated.public_material.clone();
            let opaque_key_blob = generated.opaque_blob;
            let rollback_blob = opaque_key_blob.clone();
            let characteristics = generated.characteristics;
            let result = match self.store_for_scope_mut(scope, uid, user_unlocked) {
                Ok(store) => store.register_hardware_key(
                    alias,
                    store_key_algorithm(algorithm),
                    StoreKeyFlags(flags.0),
                    &public_key,
                    &opaque_key_blob,
                    &characteristics,
                ),
                Err(status) => {
                    let _ = self.hardware.delete_key(&rollback_blob);
                    return (status, Vec::new());
                }
            };
            return match result {
                Ok(()) => (KeychainStatus::Ok, public_key),
                Err(error) => {
                    let _ = self.hardware.delete_key(&rollback_blob);
                    (status(Some(error)), Vec::new())
                }
            };
        }
        (KeychainStatus::ErrUnsupported, Vec::new())
    }

    pub fn sign(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        digest: &[u8],
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> (KeychainStatus, Vec<u8>) {
        let (hardware_backed, algorithm, flags, opaque_key_blob) = {
            let store = match self.store_for_scope_mut(scope, uid, user_unlocked) {
                Ok(store) => store,
                Err(status) => return (status, Vec::new()),
            };
            match store.key_record(alias) {
                Ok(record) => (
                    record.flags.contains(StoreKeyFlags::HARDWARE_BACKED),
                    key_algorithm_to_fidl(record.algorithm),
                    record.flags,
                    record.opaque_key_blob,
                ),
                Err(KeychainStoreError::NotFound) => (
                    false,
                    KeyAlgorithm::Ed25519,
                    StoreKeyFlags::empty(),
                    Vec::new(),
                ),
                Err(error) => return (status(Some(error)), Vec::new()),
            }
        };
        if hardware_backed {
            if flags.contains(StoreKeyFlags::REQUIRE_USER_AUTH) && hardware_auth_token.is_none() {
                return (KeychainStatus::ErrLocked, Vec::new());
            }
            return match self.hardware.sign(
                scope,
                uid,
                alias,
                algorithm,
                &opaque_key_blob,
                digest,
                hardware_auth_token,
            ) {
                Ok(signature) => (KeychainStatus::Ok, signature),
                Err(error) => (status(Some(error)), Vec::new()),
            };
        }
        let store = match self.store_for_scope_mut(scope, uid, user_unlocked) {
            Ok(store) => store,
            Err(status) => return (status, Vec::new()),
        };
        match store.sign(alias, digest) {
            Ok(signature) => (KeychainStatus::Ok, signature),
            Err(error) => (status(Some(error)), Vec::new()),
        }
    }

    pub fn aead_encrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        plaintext: &[u8],
        aad: &[u8],
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> (KeychainStatus, [u8; AES_GCM_NONCE_LEN], Vec<u8>) {
        if plaintext.len() > bexos_trusty_client::protocol::MAX_PAYLOAD_LEN || aad.len() > 64 * 1024
        {
            return (
                KeychainStatus::ErrInvalidArgs,
                [0; AES_GCM_NONCE_LEN],
                Vec::new(),
            );
        }
        let (opaque_key_blob, flags) =
            match self.require_hardware_aes_key(scope, uid, alias, user_unlocked) {
                Ok(metadata) => metadata,
                Err(status) => return (status, [0; AES_GCM_NONCE_LEN], Vec::new()),
            };
        if flags.contains(StoreKeyFlags::REQUIRE_USER_AUTH) && hardware_auth_token.is_none() {
            return (
                KeychainStatus::ErrLocked,
                [0; AES_GCM_NONCE_LEN],
                Vec::new(),
            );
        }
        match self.hardware.aead_encrypt(
            scope,
            uid,
            alias,
            &opaque_key_blob,
            plaintext,
            aad,
            hardware_auth_token,
        ) {
            Ok((nonce, ciphertext)) => (KeychainStatus::Ok, nonce, ciphertext),
            Err(error) => (status(Some(error)), [0; AES_GCM_NONCE_LEN], Vec::new()),
        }
    }

    pub fn aead_decrypt(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
        user_unlocked: bool,
        hardware_auth_token: Option<&[u8]>,
    ) -> (KeychainStatus, Vec<u8>) {
        if nonce.len() != AES_GCM_NONCE_LEN
            || ciphertext.len() < 16
            || ciphertext.len() > bexos_trusty_client::protocol::MAX_PAYLOAD_LEN + 16
            || aad.len() > 64 * 1024
        {
            return (KeychainStatus::ErrInvalidArgs, Vec::new());
        }
        let (opaque_key_blob, flags) =
            match self.require_hardware_aes_key(scope, uid, alias, user_unlocked) {
                Ok(metadata) => metadata,
                Err(status) => return (status, Vec::new()),
            };
        if flags.contains(StoreKeyFlags::REQUIRE_USER_AUTH) && hardware_auth_token.is_none() {
            return (KeychainStatus::ErrLocked, Vec::new());
        }
        match self.hardware.aead_decrypt(
            scope,
            uid,
            alias,
            &opaque_key_blob,
            nonce,
            ciphertext,
            aad,
            hardware_auth_token,
        ) {
            Ok(plaintext) => (KeychainStatus::Ok, plaintext),
            Err(error) => (status(Some(error)), Vec::new()),
        }
    }

    pub(crate) fn user_uids(&self) -> Vec<u64> {
        self.users.iter().map(|vault| vault.uid).collect()
    }

    fn validate_scope(
        &self,
        scope: KeychainScope,
        uid: u64,
        user_unlocked: bool,
    ) -> Result<(), KeychainStatus> {
        match scope {
            KeychainScope::System => Ok(()),
            KeychainScope::User if uid == 0 => Err(KeychainStatus::ErrInvalidArgs),
            KeychainScope::User if !user_unlocked => Err(KeychainStatus::ErrLocked),
            KeychainScope::User => Ok(()),
        }
    }

    fn store_for_scope_mut(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        user_unlocked: bool,
    ) -> Result<&mut VaultStore, KeychainStatus> {
        match scope {
            KeychainScope::System => {
                #[cfg(feature = "persistent")]
                snapshot::reopen(&mut self.system, self.vfsd, None)?;
                Ok(&mut self.system)
            }
            KeychainScope::User => {
                if uid == 0 {
                    return Err(KeychainStatus::ErrInvalidArgs);
                }
                if !user_unlocked {
                    return Err(KeychainStatus::ErrLocked);
                }
                if let Some(index) = self.users.iter().position(|vault| vault.uid == uid) {
                    #[cfg(feature = "persistent")]
                    snapshot::reopen(&mut self.users[index].store, self.vfsd, Some(uid))?;
                    return Ok(&mut self.users[index].store);
                }
                #[cfg(feature = "persistent")]
                let store = match self.vfsd {
                    Some(vfsd) => {
                        let db = open_user_keychain(vfsd, uid)
                            .map_err(|_| KeychainStatus::ErrStorage)?;
                        VaultStore::Persistent(db)
                    }
                    None => VaultStore::Memory(MemoryKeychainStore::new()),
                };
                #[cfg(not(feature = "persistent"))]
                let store = VaultStore::Memory(MemoryKeychainStore::new());
                self.users.push(UserVault { uid, store });
                self.users.sort_by_key(|vault| vault.uid);
                let index = self
                    .users
                    .iter()
                    .position(|vault| vault.uid == uid)
                    .unwrap();
                Ok(&mut self.users[index].store)
            }
        }
    }

    fn require_hardware_aes_key(
        &mut self,
        scope: KeychainScope,
        uid: u64,
        alias: &str,
        user_unlocked: bool,
    ) -> Result<(Vec<u8>, StoreKeyFlags), KeychainStatus> {
        let store = self.store_for_scope_mut(scope, uid, user_unlocked)?;
        let record = store
            .key_record(alias)
            .map_err(|error| status(Some(error)))?;
        if record.flags.contains(StoreKeyFlags::HARDWARE_BACKED)
            && record.algorithm == StoreKeyAlgorithm::Aes256Gcm
        {
            Ok((record.opaque_key_blob, record.flags))
        } else {
            Err(KeychainStatus::ErrUnsupported)
        }
    }
}

impl VaultStore {
    fn put_secret_record(&mut self, record: SecretRecord) -> Result<(), KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.put_secret_record(record),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.put_secret(&record),
        }
    }

    fn secret_record(&self, alias: &str) -> Result<SecretRecord, KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.secret_record(alias),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.get_secret(alias),
        }
    }

    fn store_secret(
        &mut self,
        alias: &str,
        secret: &[u8],
        flags: StoreKeyFlags,
    ) -> Result<(), KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.store_secret(alias, secret, flags),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.put_secret(&SecretRecord {
                alias: alias.into(),
                secret: secret.to_vec(),
                flags,
                generation: 0,
                envelope_key_blob: Vec::new(),
                nonce: Vec::new(),
                characteristics: Vec::new(),
            }),
        }
    }

    fn get_secret(&self, alias: &str) -> Result<Vec<u8>, KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.get_secret(alias).map(<[u8]>::to_vec),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.get_secret(alias).map(|record| record.secret),
        }
    }

    fn delete_secret(&mut self, alias: &str) -> Result<(), KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.delete_secret(alias),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.delete_secret(alias),
        }
    }

    fn generate_key(
        &mut self,
        alias: &str,
        algorithm: StoreKeyAlgorithm,
        flags: StoreKeyFlags,
    ) -> Result<Vec<u8>, KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.generate_key(alias, algorithm, flags),
            #[cfg(feature = "persistent")]
            Self::Persistent(_) => {
                bexos_keychain_store::validate_alias(alias)?;
                if flags.contains(StoreKeyFlags::HARDWARE_BACKED)
                    || algorithm != StoreKeyAlgorithm::Ed25519
                {
                    return Err(KeychainStoreError::Unsupported);
                }
                Err(KeychainStoreError::Unsupported)
            }
        }
    }

    fn register_hardware_key(
        &mut self,
        alias: &str,
        algorithm: StoreKeyAlgorithm,
        flags: StoreKeyFlags,
        public_key: &[u8],
        opaque_key_blob: &[u8],
        characteristics: &[u8],
    ) -> Result<(), KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.register_hardware_key(
                alias,
                algorithm,
                flags,
                public_key,
                opaque_key_blob,
                characteristics,
            ),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.put_hardware_key(
                alias,
                algorithm,
                flags,
                public_key,
                opaque_key_blob,
                characteristics,
            ),
        }
    }

    fn key_record(
        &self,
        alias: &str,
    ) -> Result<bexos_keychain_store::KeyRecord, KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.key_record(alias),
            #[cfg(feature = "persistent")]
            Self::Persistent(store) => store.get_key(alias),
        }
    }

    fn sign(&self, alias: &str, digest: &[u8]) -> Result<Vec<u8>, KeychainStoreError> {
        match self {
            #[cfg(feature = "persistent")]
            Self::Reopen => Err(KeychainStoreError::Storage),
            Self::Memory(store) => store.sign(alias, digest),
            #[cfg(feature = "persistent")]
            Self::Persistent(_) => {
                bexos_keychain_store::validate_alias(alias)?;
                bexos_keychain_store::validate_digest(digest)?;
                Err(KeychainStoreError::Unsupported)
            }
        }
    }
}

impl Default for KeychainService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "persistent")]
pub(crate) fn open_user_keychain(
    vfsd: Channel,
    uid: u64,
) -> Result<KeychainStoreDb, KeychainStatus> {
    let dir = vfs::get_user_home_directory(vfsd, uid).map_err(|_| KeychainStatus::ErrStorage)?;
    let result = open_keychain_file(dir);
    let close = fs::close(dir).map_err(|_| KeychainStatus::ErrStorage);
    result.and_then(|db| close.map(|()| db))
}

#[cfg(feature = "persistent")]
pub(crate) fn open_system_keychain(vfsd: Channel) -> Result<KeychainStoreDb, KeychainStatus> {
    let dir = vfs::get_system_data_directory(vfsd, KEYCHAIND_PACKAGE)
        .map_err(|_| KeychainStatus::ErrStorage)?;
    let result = open_keychain_file(dir);
    let close = fs::close(dir).map_err(|_| KeychainStatus::ErrStorage);
    result.and_then(|db| close.map(|()| db))
}

#[cfg(feature = "persistent")]
pub(crate) fn open_keychain_file(dir: Channel) -> Result<KeychainStoreDb, KeychainStatus> {
    let file =
        fs::open(dir, KEYCHAIN_DB_FILE, 1 | 2 | 8).map_err(|_| KeychainStatus::ErrStorage)?;
    let empty = fs::attributes(file)
        .map(|attributes| attributes.size_bytes == 0)
        .unwrap_or(false);
    if empty {
        let _defer_syncs = defer_file_syncs();
        return KeychainStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
            let _ = fs::close(file);
            KeychainStatus::ErrStorage
        });
    }
    KeychainStoreDb::open(FileBlockStore::new(file)).map_err(|_| {
        let _ = fs::close(file);
        KeychainStatus::ErrStorage
    })
}

pub(crate) fn status(error: Option<KeychainStoreError>) -> KeychainStatus {
    match error {
        None => KeychainStatus::Ok,
        Some(KeychainStoreError::NotFound) => KeychainStatus::ErrNotFound,
        Some(KeychainStoreError::AlreadyExists) => KeychainStatus::ErrAlreadyExists,
        Some(
            KeychainStoreError::InvalidAlias
            | KeychainStoreError::InvalidSecret
            | KeychainStoreError::InvalidDigest
            | KeychainStoreError::CorruptRecord,
        ) => KeychainStatus::ErrInvalidArgs,
        Some(KeychainStoreError::AccessDenied) => KeychainStatus::ErrAccessDenied,
        Some(KeychainStoreError::Storage) => KeychainStatus::ErrStorage,
        Some(KeychainStoreError::Unsupported) => KeychainStatus::ErrUnsupported,
    }
}

fn store_key_algorithm(algorithm: KeyAlgorithm) -> StoreKeyAlgorithm {
    match algorithm {
        KeyAlgorithm::Ed25519 => StoreKeyAlgorithm::Ed25519,
        KeyAlgorithm::EcdsaP256 => StoreKeyAlgorithm::EcdsaP256,
        KeyAlgorithm::Aes256Gcm => StoreKeyAlgorithm::Aes256Gcm,
    }
}

fn key_algorithm_to_fidl(algorithm: StoreKeyAlgorithm) -> KeyAlgorithm {
    match algorithm {
        StoreKeyAlgorithm::Ed25519 => KeyAlgorithm::Ed25519,
        StoreKeyAlgorithm::EcdsaP256 => KeyAlgorithm::EcdsaP256,
        StoreKeyAlgorithm::Aes256Gcm => KeyAlgorithm::Aes256Gcm,
    }
}

fn keymint_algorithm(algorithm: KeyAlgorithm) -> KeyMintAlgorithm {
    match algorithm {
        KeyAlgorithm::Ed25519 => KeyMintAlgorithm::Ed25519,
        KeyAlgorithm::EcdsaP256 => KeyMintAlgorithm::P256,
        KeyAlgorithm::Aes256Gcm => KeyMintAlgorithm::AesGcm,
    }
}

pub(crate) fn scope_uid(scope: KeychainScope, uid: u64) -> u64 {
    match scope {
        KeychainScope::System => 1,
        KeychainScope::User => uid,
    }
}

fn keychain_flags_contain(flags: KeyFlags, bit: u16) -> bool {
    flags.0 & bit == bit
}

fn secret_envelope_aad(scope: KeychainScope, uid: u64, alias: &str) -> Vec<u8> {
    let mut aad = b"bexos.keychain.secret.v1\0".to_vec();
    aad.push(match scope {
        KeychainScope::System => 1,
        KeychainScope::User => 2,
    });
    aad.extend_from_slice(&uid.to_le_bytes());
    aad.extend_from_slice(alias.as_bytes());
    aad
}

fn tee_open_keymint_session(client: Channel) -> Result<u64, KeychainStoreError> {
    let response = tee_call(
        client,
        5,
        &tee::TeeManagerOpenSessionRequest { uuid: KEYMINT_UUID },
    )?;
    let decoded = tee::TeeManagerOpenSessionResponse::decode(&response.bytes, &response.handles)
        .map_err(|_| KeychainStoreError::Storage)?;
    if decoded.status != tee::TeeStatus::Ok {
        return Err(KeychainStoreError::Unsupported);
    }
    Ok(decoded.session_id)
}

fn tee_invoke(
    client: Channel,
    session_id: u64,
    command_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, KeychainStoreError> {
    let payload_vmo = Memory::from_bytes(payload).map_err(|_| KeychainStoreError::Storage)?;
    let call = tee_call(
        client,
        7,
        &tee::TeeManagerInvokeCommandRequest {
            session_id,
            command_id,
            payload: tee::HandleRef { raw: payload_vmo },
            payload_len: payload.len() as u64,
        },
    );
    let response = call?;
    let decoded =
        match tee::TeeManagerInvokeCommandResponse::decode(&response.bytes, &response.handles) {
            Ok(decoded) => decoded,
            Err(_) => {
                for handle in response.handles {
                    let _ = Memory::close(handle.raw);
                }
                return Err(KeychainStoreError::Storage);
            }
        };
    if decoded.status != tee::TeeStatus::Ok {
        let _ = Memory::close(decoded.response.raw);
        return Err(KeychainStoreError::AccessDenied);
    }
    if decoded.response_len == 0 || decoded.response_len > 64 * 1024 {
        let _ = Memory::close(decoded.response.raw);
        return Err(KeychainStoreError::Storage);
    }
    let rounded = decoded
        .response_len
        .checked_add(4095)
        .map(|len| len & !4095)
        .ok_or(KeychainStoreError::Storage)?;
    let va =
        Memory::map(decoded.response.raw, rounded, 2).map_err(|_| KeychainStoreError::Storage)?;
    let bytes =
        unsafe { core::slice::from_raw_parts(va as *const u8, decoded.response_len as usize) }
            .to_vec();
    Memory::unmap(va, rounded).map_err(|_| KeychainStoreError::Storage)?;
    Memory::close(decoded.response.raw).map_err(|_| KeychainStoreError::Storage)?;
    Ok(bytes)
}

struct TeeResponse {
    bytes: Vec<u8>,
    handles: Vec<tee::HandleRef>,
}

fn tee_call<Q: TeeFidlEncode>(
    channel: Channel,
    ordinal: u64,
    request: &Q,
) -> Result<TeeResponse, KeychainStoreError> {
    let mut bytes = vec![0; 65500];
    let mut handles = [tee::HandleRef { raw: 0 }; 8];
    let encoded = request
        .encode(&mut bytes, &mut handles)
        .map_err(|_| KeychainStoreError::Storage)?;
    let message = Rpc(channel)
        .call_raw(
            ordinal,
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
            true,
        )
        .map_err(|_| KeychainStoreError::Storage)?;
    Ok(TeeResponse {
        bytes: message.bytes,
        handles: message
            .handles
            .iter()
            .map(|raw| tee::HandleRef { raw: *raw })
            .collect(),
    })
}
