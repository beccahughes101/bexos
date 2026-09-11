//! Replaceable scheduling code cannot own a guest execution transaction. Its
//! versioned call returns bounded work to the permanent execution nucleus.
use crate::image::{Image, ImageError};

pub const BASE: usize = 0x04000000;
pub const BYTES: usize = 0x200000;
pub const END: usize = BASE + BYTES;
pub const STACK: usize = END - 0x10000;
pub const DESCRIPTOR: usize = BASE + 0x1000;
pub const MAGIC: u64 = u64::from_le_bytes(*b"BEXMP001");
pub const CALL: u64 = u64::from_le_bytes(*b"BEXPC001");
pub const RESPONSE: u64 = 0x42585031;
pub const FAILURE: u64 = u64::MAX;

pub const fn descriptor(tag: u64) -> [u64; 8] {
    [MAGIC, 1, 2, BASE as u64, END as u64, STACK as u64, tag, 0]
}
pub fn validate(image: &Image<'_>) -> Result<u8, ImageError> {
    image.require_identity_linked()?;
    image.reserve(0x200000, BASE - 0x200000)?;
    if image.entry() != BASE as u64 {
        return Err(ImageError::Entry);
    }
    image.executable_at(BASE, 1)?;
    image.reserve(STACK, END - STACK)?;
    let bytes = image.bytes_at(DESCRIPTOR, 64)?;
    let tag = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
    if tag == 0 || tag > u8::MAX as u64 {
        return Err(ImageError::Header);
    }
    for (actual, expected) in bytes.chunks_exact(8).zip(descriptor(tag)) {
        if actual != expected.to_le_bytes() {
            return Err(ImageError::Header);
        }
    }
    Ok(tag as u8)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Work {
    pub normal: u8,
    pub secure: u8,
}
impl Work {
    pub fn decode(reply: u64, tag: u8) -> Option<Self> {
        let normal = reply as u8;
        let secure = (reply >> 8) as u8;
        (reply >> 32 == RESPONSE
            && (reply >> 24) as u8 == tag
            && (reply >> 16) as u8 == 0
            && normal <= 32
            && (1..=4).contains(&secure))
        .then_some(Self { normal, secure })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> [u8; 512] {
        let mut b = [0; 512];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        b[16..18].copy_from_slice(&2u16.to_le_bytes());
        b[18..20].copy_from_slice(&62u16.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[24..32].copy_from_slice(&(BASE as u64).to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&2u16.to_le_bytes());
        for (i, (address, offset, flags, length)) in
            [(BASE, 256u64, 5u32, 16u64), (DESCRIPTOR, 320, 4, 64)]
                .into_iter()
                .enumerate()
        {
            let at = 64 + i * 56;
            b[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
            b[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
            for (field, value) in [
                (8, offset),
                (16, address as u64),
                (24, address as u64),
                (32, length),
                (40, length),
                (48, 1),
            ] {
                b[at + field..at + field + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        for (target, value) in b[320..384].chunks_exact_mut(8).zip(descriptor(2)) {
            target.copy_from_slice(&value.to_le_bytes());
        }
        b
    }
    #[test]
    fn versioned_candidate_cannot_load_outside_private_code_and_stack_bank() {
        let b = fixture();
        assert_eq!(validate(&Image::parse(&b, END).unwrap()), Ok(2));
        for word in [0, 1, 2, 3, 4, 5, 7] {
            let mut bad = b;
            bad[320 + word * 8] ^= 1;
            assert!(validate(&Image::parse(&bad, END).unwrap()).is_err());
        }
        let mut bad = b;
        bad[56..58].copy_from_slice(&3u16.to_le_bytes());
        let at = 64 + 2 * 56;
        bad[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        // A trailing BSS segment must not overwrite permanent state or the
        // candidate's guarded stack, even though the entry remains valid.
        for address in [0x200000u64, STACK as u64, 0x18000000] {
            bad[at + 16..at + 24].copy_from_slice(&address.to_le_bytes());
            bad[at + 24..at + 32].copy_from_slice(&address.to_le_bytes());
            bad[at + 40..at + 48].copy_from_slice(&1u64.to_le_bytes());
            assert!(
                Image::parse(&bad, END)
                    .and_then(|image| validate(&image))
                    .is_err()
            );
        }
        let mut bad = b;
        bad[68..72].copy_from_slice(&7u32.to_le_bytes());
        assert!(validate(&Image::parse(&bad, END).unwrap()).is_err());
    }
    #[test]
    fn candidate_cannot_request_unbounded_or_incompatible_execution() {
        let valid = (RESPONSE << 32) | (2 << 24) | (1 << 8) | 16;
        assert_eq!(
            Work::decode(valid, 2),
            Some(Work {
                normal: 16,
                secure: 1
            })
        );
        for bad in [
            FAILURE,
            valid ^ (1 << 32),
            valid | (1 << 16),
            valid ^ (1 << 24),
            valid | 63,
            valid & !0xff00,
            valid | (8 << 8),
        ] {
            assert_eq!(Work::decode(bad, 2), None);
        }
    }
}
