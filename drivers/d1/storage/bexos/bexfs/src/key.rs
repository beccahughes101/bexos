use alloc::vec::Vec;
use core::ops::Deref;

pub const VOLUME_KEY_BYTES: usize = 32;

pub trait VolumeKeyProvider {
    fn unlock(&self, volume_uuid: [u8; 16]) -> Result<LockedVolumeKey, KeyError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyError {
    Unavailable,
    InvalidLength,
    InvalidWrappedKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrappedVolumeKey {
    pub volume_uuid: [u8; 16],
    pub wrapped_key: Vec<u8>,
}

pub trait WrappedVolumeKeyProvider {
    fn unwrap(&self, wrapped: &WrappedVolumeKey) -> Result<LockedVolumeKey, KeyError>;
}

pub struct LockedVolumeKey([u8; VOLUME_KEY_BYTES]);

impl LockedVolumeKey {
    pub fn new(bytes: &[u8]) -> Result<Self, KeyError> {
        let material: [u8; VOLUME_KEY_BYTES] =
            bytes.try_into().map_err(|_| KeyError::InvalidLength)?;
        Ok(Self(material))
    }

    pub const fn expose(&self) -> &[u8; VOLUME_KEY_BYTES] {
        &self.0
    }
}

impl Deref for LockedVolumeKey {
    type Target = [u8; VOLUME_KEY_BYTES];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for LockedVolumeKey {
    fn drop(&mut self) {
        rosefs_core::keys::zeroize(&mut self.0);
    }
}
