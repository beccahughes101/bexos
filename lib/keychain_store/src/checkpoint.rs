//! The in-memory store uses the same record codec as the durable backend.
use super::*;

impl MemoryKeychainStore {
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, 1, 1);
        put_varint(&mut out, 2, self.generation);
        for secret in &self.secrets {
            put_bytes(&mut out, 3, &encode_secret(secret));
        }
        for key in &self.keys {
            put_bytes(&mut out, 4, &encode_key(key));
        }
        out
    }

    pub fn from_checkpoint(bytes: &[u8]) -> Result<Self, KeychainStoreError> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(KeychainStoreError::CorruptRecord);
        }
        let mut store = Self::new();
        let mut version = None;
        let mut generation = None;
        read_fields(bytes, |field| {
            match field.number {
                1 if version.is_none() => version = Some(field.varint()?),
                2 if generation.is_none() => generation = Some(field.varint()?),
                3 if store.secrets.len() < 4096 => {
                    let secret = decode_secret(field.bytes()?)?;
                    if store.secrets.iter().any(|old| old.alias == secret.alias) {
                        return Err(KeychainStoreError::CorruptRecord);
                    }
                    store.secrets.push(secret);
                }
                4 if store.keys.len() < 4096 => {
                    let key = decode_key(field.bytes()?)?;
                    if store.keys.iter().any(|old| old.alias == key.alias) {
                        return Err(KeychainStoreError::CorruptRecord);
                    }
                    store.keys.push(key);
                }
                _ => return Err(KeychainStoreError::CorruptRecord),
            }
            Ok(())
        })?;
        if version != Some(1) {
            return Err(KeychainStoreError::CorruptRecord);
        }
        store.generation = generation.ok_or(KeychainStoreError::CorruptRecord)?;
        if store
            .secrets
            .iter()
            .any(|entry| entry.generation > store.generation)
            || store
                .keys
                .iter()
                .any(|entry| entry.generation > store.generation)
        {
            return Err(KeychainStoreError::CorruptRecord);
        }
        Ok(store)
    }
}
