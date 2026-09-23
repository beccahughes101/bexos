//! Versioned monitor entry contract. This validates layout after independent
//! firmware authentication; it does not grant permission to execute an ELF.
use crate::image::{Image, ImageError};
pub const IMAGE_BASE: usize = 0x04000000;
pub const IMAGE_END: usize = 0x18000000;
pub const IMAGE_BYTES: usize = IMAGE_END - IMAGE_BASE;
// Q35 splits 3 GiB into 2 GiB below the PCI hole and 1 GiB above 4 GiB.
// 0x6000_0000..0x7000_0000 is the second Trusty execution bank; keep the
// second monitor image in the high-RAM aperture so both can be replaced.
pub const INACTIVE_BANK: usize = 0x1_0000_0000;
pub const RETIRING_ALIAS: usize = 0xa0000000;
pub const RESIDENT_END: usize = 0x18200000;
pub const RESUME_ENTRY: usize = 0x04001000;
pub const DESCRIPTOR: usize = 0x04002000;
pub const MAGIC: u64 = u64::from_le_bytes(*b"BEXMR001");
pub const fn descriptor(state_bytes: usize) -> [u64; 8] {
    [
        MAGIC,
        1,
        2,
        IMAGE_BASE as u64,
        IMAGE_BYTES as u64,
        RESIDENT_END as u64,
        state_bytes as u64,
        RESUME_ENTRY as u64,
    ]
}
pub fn validate(image: &Image<'_>, state_bytes: usize) -> Result<(), ImageError> {
    let bytes = image.bytes_at(DESCRIPTOR, 64)?;
    for (chunk, expected) in bytes.chunks_exact(8).zip(descriptor(state_bytes)) {
        if chunk != expected.to_le_bytes() {
            return Err(ImageError::Header);
        }
    }
    image.executable_at(RESUME_ENTRY, 16)?;
    Ok(())
}
