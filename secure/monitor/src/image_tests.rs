extern crate std;
use super::*;
use std::vec;
use std::vec::Vec;
fn fixture() -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    put(&mut bytes, 24, 0xffff_ffff_8000_0000);
    put(&mut bytes, 32, 64);
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
    put(&mut bytes, 72, 256);
    put(&mut bytes, 80, 0xffff_ffff_8000_0000);
    put(&mut bytes, 88, 0x200000);
    put(&mut bytes, 96, 8);
    put(&mut bytes, 104, 32);
    put(&mut bytes, 112, 1);
    bytes[256..264].fill(0x5a);
    bytes
}
fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn incremental_preparation_clears_only_its_chunk_and_preserves_segment_edges() {
    let mut bytes = fixture();
    put(&mut bytes, 24, 0x200000);
    put(&mut bytes, 80, 0x200000);
    let image = Image::parse(&bytes, 0x300000).unwrap();
    for chunk in [1usize, 3, 7, 8, 9, 31, 32] {
        let mut bank = [0xa5; 32];
        for offset in (0..32).step_by(chunk) {
            let length = chunk.min(32 - offset);
            image
                .load_replaceable_chunk(0x200000, &mut bank, 0x210000..0x300000, offset, length)
                .unwrap();
            assert!(bank[offset + length..].iter().all(|b| *b == 0xa5));
        }
        assert_eq!(&bank[..8], &[0x5a; 8]);
        assert_eq!(&bank[8..], &[0; 24]);
    }
    for (offset, length) in [(32usize, 1usize), (usize::MAX, 1), (0, 0)] {
        let mut bank = [0xa5; 32];
        assert!(
            image
                .load_replaceable_chunk(0x200000, &mut bank, 0x210000..0x300000, offset, length)
                .is_err()
        );
        assert_eq!(bank, [0xa5; 32]);
    }
}
#[test]
fn checked_load_resolves_entry_copies_files_and_clears_bss() {
    let bytes = fixture();
    let image = Image::parse(&bytes, 0x200020).unwrap();
    assert_eq!(image.entry(), 0x200000);
    let mut bank = vec![0xa5; 0x200020];
    image.load(&mut bank).unwrap();
    assert!(bank[..0x200000].iter().all(|b| *b == 0xa5));
    assert_eq!(&bank[0x200000..0x200008], &[0x5a; 8]);
    assert_eq!(&bank[0x200008..], &[0; 24]);
    assert_eq!(
        image.load(&mut bank[..0x200010]),
        Err(ImageError::Destination)
    );
    let mut bytes = fixture();
    put(&mut bytes, 24, 0x200000);
    assert_eq!(Image::parse(&bytes, 0x200020).unwrap().entry(), 0x200000);
}
#[test]
fn malformed_headers_segments_and_entries_are_rejected() {
    let good = fixture();
    for length in 0..120 {
        assert!(Image::parse(&good[..length], 0x200020).is_err());
    }
    for (offset, value) in [
        (32, u64::MAX),
        (72, u64::MAX),
        (88, 0x100000),
        (88, u64::MAX),
        (96, 33),
        (104, u64::MAX),
        (112, 3),
        (24, 0x200008),
    ] {
        let mut bytes = fixture();
        put(&mut bytes, offset, value);
        assert!(Image::parse(&bytes, 0x200020).is_err());
    }
    let mut bytes = fixture();
    bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
    assert!(matches!(
        Image::parse(&bytes, 0x200020),
        Err(ImageError::Architecture)
    ));
    let mut bytes = fixture();
    bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
    let segment: Vec<u8> = bytes[64..120].to_vec();
    bytes[120..176].copy_from_slice(&segment);
    assert!(matches!(
        Image::parse(&bytes, 0x200020),
        Err(ImageError::Overlap)
    ));
}

#[test]
fn loader_reservations_reject_segments_and_bss_without_writing_memory() {
    let bytes = fixture();
    let image = Image::parse(&bytes, 0x300000).unwrap();
    // Exact adjacent ranges are permitted. A zero-filled tail is still owned
    // by the ELF and must not overlap evidence or another loader reservation.
    assert_eq!(image.reserve(0x1ff000, 0x1000), Ok(()));
    assert_eq!(image.reserve(0x200020, 0x1000), Ok(()));
    for (start, length) in [(0x200000, 1), (0x200008, 1), (0x1fffff, 2), (0x20001f, 2)] {
        assert_eq!(image.reserve(start, length), Err(ImageError::Overlap));
    }
    for (start, length) in [(0, 0), (usize::MAX, 1), (0x300000, 1), (1, usize::MAX)] {
        assert_eq!(image.reserve(start, length), Err(ImageError::Range));
    }
}

#[test]
fn inactive_region_checks_every_segment_before_copying() {
    let mut bytes = fixture();
    bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
    let segment = bytes[64..120].to_vec();
    bytes[120..176].copy_from_slice(&segment);
    bytes[124..128].copy_from_slice(&6u32.to_le_bytes());
    put(&mut bytes, 136, 0x200040);
    put(&mut bytes, 144, 0x200040);
    let image = Image::parse(&bytes, 0x300000).unwrap();
    let mut bank = [0xa5; 96];
    // The second segment is outside the bank. The valid first segment must
    // remain untouched when the whole candidate cannot be installed.
    assert_eq!(
        image.load_region(0x200000, &mut bank[..64]),
        Err(ImageError::Destination)
    );
    assert_eq!(bank, [0xa5; 96]);
    assert_eq!(
        image.load_region(0x200001, &mut bank),
        Err(ImageError::Destination)
    );
    assert_eq!(
        image.load_region(usize::MAX, &mut bank),
        Err(ImageError::Destination)
    );
    assert_eq!(bank, [0xa5; 96]);
    image.load_region(0x200000, &mut bank).unwrap();
    assert_eq!(&bank[..8], &[0x5a; 8]);
    assert_eq!(&bank[8..32], &[0; 24]);
    assert_eq!(&bank[32..64], &[0xa5; 32]);
    assert_eq!(&bank[64..72], &[0x5a; 8]);
    assert_eq!(&bank[72..], &[0; 24]);
}

#[test]
fn root_candidates_require_identity_linked_segments() {
    let mut bytes = fixture();
    assert_eq!(
        Image::parse(&bytes, 0x300000)
            .unwrap()
            .require_identity_linked(),
        Err(ImageError::Destination)
    );
    put(&mut bytes, 24, 0x200000);
    put(&mut bytes, 80, 0x200000);
    assert_eq!(
        Image::parse(&bytes, 0x300000)
            .unwrap()
            .require_identity_linked(),
        Ok(())
    );
}

#[test]
fn replaceable_loading_retains_resident_bytes_and_rejects_trailing_escape_atomically() {
    let mut bytes = fixture();
    put(&mut bytes, 24, 0x200000);
    put(&mut bytes, 80, 0x200000);
    bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
    let segment = bytes[64..120].to_vec();
    bytes[120..176].copy_from_slice(&segment);
    put(&mut bytes, 136, 0x300000);
    put(&mut bytes, 144, 0x300000);
    put(&mut bytes, 160, 8);
    let image = Image::parse(&bytes, 0x400000).unwrap();
    let mut bank = [0xa5; 64];
    image.require_resident_code(0x300000, &[0x5a; 8]).unwrap();
    assert!(image.require_resident_code(0x300000, &[0x5b; 8]).is_err());
    image
        .load_replaceable(0x200000, &mut bank, 0x300000..0x301000)
        .unwrap();
    assert_eq!(&bank[..8], &[0x5a; 8]);
    assert_eq!(&bank[8..], &[0; 56]);
    // A final segment crossing the retained boundary cannot copy even the
    // earlier valid code, and cannot overwrite the resident recovery nucleus.
    put(&mut bytes, 136, 0x2ffff0);
    put(&mut bytes, 144, 0x2ffff0);
    put(&mut bytes, 160, 32);
    let image = Image::parse(&bytes, 0x400000).unwrap();
    bank.fill(0xa5);
    assert!(
        image
            .load_replaceable(0x200000, &mut bank, 0x300000..0x301000)
            .is_err()
    );
    assert_eq!(bank, [0xa5; 64]);
}

#[test]
fn resume_descriptor_is_file_backed_and_matches_the_installed_state_abi() {
    use crate::monitor_image as monitor;
    let mut bytes = fixture();
    bytes.resize(256 + 0x3000, 0);
    put(&mut bytes, 24, monitor::IMAGE_BASE as u64);
    put(&mut bytes, 80, monitor::IMAGE_BASE as u64);
    put(&mut bytes, 88, monitor::IMAGE_BASE as u64);
    put(&mut bytes, 96, 0x3000);
    put(&mut bytes, 104, 0x4000);
    let note = 256 + monitor::DESCRIPTOR - monitor::IMAGE_BASE;
    for (chunk, value) in bytes[note..note + 64]
        .chunks_exact_mut(8)
        .zip(monitor::descriptor(4096))
    {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    let image = Image::parse(&bytes, monitor::RESIDENT_END).unwrap();
    monitor::validate(&image, 4096).unwrap();
    assert!(monitor::validate(&image, 4097).is_err());
    assert!(image.bytes_at(monitor::IMAGE_BASE + 0x3000, 1).is_err());
    bytes[68] = 7;
    assert!(
        monitor::validate(&Image::parse(&bytes, monitor::RESIDENT_END).unwrap(), 4096).is_err()
    );
    bytes[68] = 5;
    bytes[note] ^= 1;
    assert!(
        monitor::validate(&Image::parse(&bytes, monitor::RESIDENT_END).unwrap(), 4096).is_err()
    );
}
