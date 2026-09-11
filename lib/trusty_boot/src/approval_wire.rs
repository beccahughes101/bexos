//! Retained boot approval, independent of a Rust object's layout or address.
//! The record carries no signature: only its protected origin is authority.
use super::{Approval, Error};

pub const STATE_BYTES: usize = 32;

impl Approval {
    pub fn snapshot(self) -> [u8; STATE_BYTES] {
        let mut output = [0; STATE_BYTES];
        output[..8].copy_from_slice(b"BEXAP001");
        output[8..12].copy_from_slice(&self.location.to_le_bytes());
        output[16..24].copy_from_slice(&self.generation.to_le_bytes());
        output[24..32].copy_from_slice(&self.floor.to_le_bytes());
        output
    }

    /// # Safety
    /// The boot owner must have captured this record after successful Trusty
    /// approval and retained it exclusively in protected memory. Never call
    /// this on guest-supplied data, even if its checksum or shape is valid.
    pub unsafe fn restore_protected(input: &[u8]) -> Result<Self, Error> {
        if input.len() != STATE_BYTES || &input[..8] != b"BEXAP001" || input[12..16] != [0; 4] {
            return Err(Error::InvalidGeneration);
        }
        let location = u32::from_le_bytes(input[8..12].try_into().unwrap());
        let generation = u64::from_le_bytes(input[16..24].try_into().unwrap());
        let floor = u64::from_le_bytes(input[24..32].try_into().unwrap());
        if location > 31 || generation == 0 || floor > generation {
            return Err(Error::InvalidGeneration);
        }
        Ok(Self {
            generation,
            floor,
            location,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_approval_preserves_floor_and_rejects_invalid_records() {
        let approved = Approval {
            generation: 8,
            floor: 7,
            location: 31,
        };
        let bytes = approved.snapshot();
        assert_eq!(unsafe { Approval::restore_protected(&bytes) }, Ok(approved));
        for length in 0..STATE_BYTES {
            assert!(unsafe { Approval::restore_protected(&bytes[..length]) }.is_err());
        }
        for (offset, value) in [(0, 0), (7, b'2'), (8, 32), (12, 1), (16, 0), (24, 9)] {
            let mut invalid = bytes;
            invalid[offset] = value;
            assert!(unsafe { Approval::restore_protected(&invalid) }.is_err());
        }
        let mut longer = [0; STATE_BYTES + 1];
        longer[..STATE_BYTES].copy_from_slice(&bytes);
        assert!(unsafe { Approval::restore_protected(&longer) }.is_err());
    }
}
