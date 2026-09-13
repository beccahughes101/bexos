//! Registry secrets deliberately have no Debug or serialization implementation.
use crate::{Error, Result};
use std::collections::BTreeMap;
pub struct Secret(Vec<u8>);
impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}
pub struct Credential {
    pub token: Secret,
    pub certificates: Vec<Vec<u8>>,
    pub private_key: Secret,
    pub sealed: bool,
    pub generation: u64,
}
#[derive(Default)]
pub struct Vault {
    pub(crate) entries: BTreeMap<String, Credential>,
    generation: u64,
}
impl Vault {
    pub(crate) fn next_generation(&mut self) -> Result<u64> {
        // Restored credentials and mTLS replacements must participate in the
        // same sequence as token changes, so live transfers cannot reuse an epoch.
        let latest = self
            .entries
            .values()
            .map(|c| c.generation)
            .max()
            .unwrap_or(0);
        self.generation = self
            .generation
            .max(latest)
            .checked_add(1)
            .ok_or(Error::ResourceExhausted)?;
        Ok(self.generation)
    }
    pub fn set_token(&mut self, host: &str, token: Vec<u8>, sealed: bool) -> Result<()> {
        if !bexos_pkg_client::valid_host(host)
            || token.is_empty()
            || token.len() > 1024
            || !token.iter().all(|b| (33..=126).contains(b))
        {
            return Err(Error::InvalidArgs);
        }
        let generation = self.next_generation()?;
        if let Some(credential) = self.entries.get_mut(host) {
            credential.token = Secret::new(token);
            credential.sealed = sealed;
            credential.generation = generation;
        } else {
            self.entries.insert(
                host.into(),
                Credential {
                    token: Secret::new(token),
                    certificates: Vec::new(),
                    private_key: Secret::new(Vec::new()),
                    sealed,
                    generation,
                },
            );
        }
        Ok(())
    }
    pub fn remove(&mut self, host: &str) {
        self.entries.remove(host);
        self.generation = self.generation.saturating_add(1);
    }
    pub fn get(&self, host: &str) -> Option<&Credential> {
        self.entries.get(host)
    }
}
