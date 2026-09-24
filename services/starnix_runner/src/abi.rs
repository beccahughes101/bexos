use crate::vfs::Stat;
use starnix_kernel::{Architecture, EFAULT, EINVAL};

pub fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, i64> {
    Ok(u32::from_ne_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(EFAULT)?
            .try_into()
            .unwrap(),
    ))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, i64> {
    Ok(u64::from_ne_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(EFAULT)?
            .try_into()
            .unwrap(),
    ))
}

pub fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
}

pub fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

pub fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

pub fn put_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

pub fn encode_stat(architecture: Architecture, stat: &Stat) -> Vec<u8> {
    match architecture {
        Architecture::X86_64 => {
            let mut out = vec![0; 144];
            put_u64(&mut out, 0, 1);
            put_u64(&mut out, 8, stat.inode);
            put_u64(&mut out, 16, stat.links);
            put_u32(&mut out, 24, stat.mode);
            put_u32(&mut out, 28, 0);
            put_u32(&mut out, 32, 0);
            put_i64(&mut out, 48, stat.size as i64);
            put_i64(&mut out, 56, 4096);
            put_i64(&mut out, 64, stat.blocks as i64);
            timestamp(&mut out, 72, stat.modified);
            timestamp(&mut out, 88, stat.modified);
            timestamp(&mut out, 104, stat.created);
            out
        }
        Architecture::Aarch64 => {
            let mut out = vec![0; 128];
            put_u64(&mut out, 0, 1);
            put_u64(&mut out, 8, stat.inode);
            put_u32(&mut out, 16, stat.mode);
            put_u32(&mut out, 20, stat.links as u32);
            put_u32(&mut out, 24, 0);
            put_u32(&mut out, 28, 0);
            put_i64(&mut out, 48, stat.size as i64);
            put_i64(&mut out, 56, 4096);
            put_i64(&mut out, 64, stat.blocks as i64);
            timestamp(&mut out, 72, stat.modified);
            timestamp(&mut out, 88, stat.modified);
            timestamp(&mut out, 104, stat.created);
            out
        }
    }
}

fn timestamp(out: &mut [u8], offset: usize, nanos: u64) {
    put_i64(out, offset, (nanos / 1_000_000_000) as i64);
    put_u64(out, offset + 8, nanos % 1_000_000_000);
}

pub fn encode_timespec(nanos: u64) -> [u8; 16] {
    let mut out = [0; 16];
    put_u64(&mut out, 0, nanos / 1_000_000_000);
    put_u64(&mut out, 8, nanos % 1_000_000_000);
    out
}

pub fn decode_timespec(bytes: &[u8]) -> Result<u64, i64> {
    let seconds = read_u64(bytes, 0)?;
    let nanos = read_u64(bytes, 8)?;
    if nanos >= 1_000_000_000 {
        return Err(EINVAL);
    }
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(nanos))
        .ok_or(EINVAL)
}

pub fn encode_uts() -> [u8; 390] {
    let mut out = [0; 390];
    for (index, value) in [
        "Linux",
        "bexos",
        "6.1.0-bexos",
        "#1 BexOS Starnix",
        if cfg!(bexos_arch_x86_64) {
            "x86_64"
        } else {
            "aarch64"
        },
        "(none)",
    ]
    .iter()
    .enumerate()
    {
        let start = index * 65;
        out[start..start + value.len()].copy_from_slice(value.as_bytes());
    }
    out
}

pub fn encode_dirents(
    entries: &[crate::vfs::DirectoryEntry],
    maximum: usize,
    starting_offset: usize,
) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut encoded = 0;
    for (offset, entry) in entries.iter().enumerate() {
        let record = (19 + entry.name.len() + 1).div_ceil(8) * 8;
        if out.len() + record > maximum {
            break;
        }
        let start = out.len();
        out.resize(start + record, 0);
        put_u64(&mut out, start, entry.inode);
        put_i64(
            &mut out,
            start + 8,
            starting_offset.saturating_add(offset).saturating_add(1) as i64,
        );
        put_u16(&mut out, start + 16, record as u16);
        out[start + 18] = entry.kind;
        out[start + 19..start + 19 + entry.name.len()].copy_from_slice(entry.name.as_bytes());
        encoded += 1;
    }
    (out, encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_layouts_have_linux_sizes() {
        let stat = Stat {
            inode: 7,
            mode: 0o100644,
            links: 1,
            size: 9,
            blocks: 1,
            created: 2,
            modified: 3,
            kind: 8,
        };
        assert_eq!(encode_stat(Architecture::X86_64, &stat).len(), 144);
        assert_eq!(encode_stat(Architecture::Aarch64, &stat).len(), 128);
    }

    #[test]
    fn dirents_are_aligned_and_bounded() {
        let entries = [crate::vfs::DirectoryEntry {
            inode: 1,
            name: "abc".into(),
            kind: 8,
        }];
        let (bytes, count) = encode_dirents(&entries, 64, 0);
        assert_eq!(bytes.len() % 8, 0);
        assert!(bytes.len() <= 64);
        assert_eq!(count, 1);
    }
}
