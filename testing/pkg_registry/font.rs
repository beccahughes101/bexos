//! Give the pinned OFL fixture a distinct family so guest local fonts cannot mask a miss.
pub fn remote_fixture(mut bytes: Vec<u8>) -> Vec<u8> {
    let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
    let mut name = None;
    let mut head = None;
    for table in 0..count {
        let row = 12 + table * 16;
        let offset = u32::from_be_bytes(bytes[row + 8..row + 12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(bytes[row + 12..row + 16].try_into().unwrap()) as usize;
        if &bytes[row..row + 4] == b"name" {
            name = Some((offset, length));
        }
        if &bytes[row..row + 4] == b"head" {
            head = Some(offset);
        }
    }
    let (start, length) = name.unwrap();
    let old = b"\0I\0n\0t\0e\0r";
    let new = b"\0P\0r\0o\0b\0e";
    let mut replacements = 0;
    for offset in start..start + length - old.len() + 1 {
        if &bytes[offset..offset + old.len()] == old {
            bytes[offset..offset + old.len()].copy_from_slice(new);
            replacements += 1;
        }
    }
    assert!(replacements > 0);
    let head = head.unwrap();
    bytes[head + 8..head + 12].fill(0);
    for table in 0..count {
        let row = 12 + table * 16;
        let offset = u32::from_be_bytes(bytes[row + 8..row + 12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(bytes[row + 12..row + 16].try_into().unwrap()) as usize;
        let sum = checksum(&bytes[offset..offset + length]);
        bytes[row + 4..row + 8].copy_from_slice(&sum.to_be_bytes());
    }
    let adjustment = 0xb1b0_afbau32.wrapping_sub(checksum(&bytes));
    bytes[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    bytes
}
fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}
