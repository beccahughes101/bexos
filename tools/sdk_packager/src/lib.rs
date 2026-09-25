use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

pub fn archive(root: &str, entries: &[Entry]) -> Result<Vec<u8>, String> {
    validate_path(root)?;
    let mut sorted = BTreeMap::new();
    for entry in entries {
        validate_path(&entry.path)?;
        if sorted.insert(entry.path.clone(), entry).is_some() {
            return Err(format!("duplicate SDK path {}", entry.path));
        }
    }

    let mut tar = Vec::new();
    for (path, entry) in sorted {
        let archive_path = format!("{root}/{path}");
        append_tar_file(
            &mut tar,
            &archive_path,
            &entry.bytes,
            if entry.executable { 0o755 } else { 0o644 },
        )?;
    }
    tar.resize(tar.len() + 1024, 0);
    Ok(gzip_store(&tar))
}

pub fn sha256_line(name: &str, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64 + 2 + name.len() + 1);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out.push_str("  ");
    out.push_str(name);
    out.push('\n');
    out
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("unsafe SDK path {path:?}"));
    }
    Ok(())
}

fn append_tar_file(out: &mut Vec<u8>, path: &str, bytes: &[u8], mode: u32) -> Result<(), String> {
    let (name, prefix) = split_ustar_path(path)?;
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut header[100..108], u64::from(mode));
    octal(&mut header[108..116], 0);
    octal(&mut header[116..124], 0);
    octal(&mut header[124..136], bytes.len() as u64);
    octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[265..270].copy_from_slice(b"root\0");
    header[297..302].copy_from_slice(b"root\0");
    header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
    octal_checksum(&mut header[148..156], checksum);
    out.extend_from_slice(&header);
    out.extend_from_slice(bytes);
    let padding = (512 - bytes.len() % 512) % 512;
    out.resize(out.len() + padding, 0);
    Ok(())
}

fn split_ustar_path(path: &str) -> Result<(&str, &str), String> {
    if path.len() <= 100 {
        return Ok((path, ""));
    }
    for (index, _) in path.match_indices('/').rev() {
        let prefix = &path[..index];
        let name = &path[index + 1..];
        if prefix.len() <= 155 && name.len() <= 100 {
            return Ok((name, prefix));
        }
    }
    Err(format!("SDK archive path exceeds ustar limits: {path}"))
}

fn octal(field: &mut [u8], value: u64) {
    field.fill(b'0');
    field[field.len() - 1] = 0;
    let text = format!("{value:o}");
    let start = field.len() - 1 - text.len();
    field[start..start + text.len()].copy_from_slice(text.as_bytes());
}

fn octal_checksum(field: &mut [u8], value: u64) {
    let text = format!("{value:06o}");
    field[..6].copy_from_slice(text.as_bytes());
    field[6] = 0;
    field[7] = b' ';
}

fn gzip_store(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    if bytes.is_empty() {
        out.push(1);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&u16::MAX.to_le_bytes());
    } else {
        let chunks = bytes.chunks(u16::MAX as usize).collect::<Vec<_>>();
        for (index, chunk) in chunks.iter().enumerate() {
            out.push(u8::from(index + 1 == chunks.len()));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(&crc32(bytes).to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_is_deterministic_and_sorted() {
        let a = Entry {
            path: "z".into(),
            bytes: b"last".to_vec(),
            executable: true,
        };
        let b = Entry {
            path: "a".into(),
            bytes: b"first".to_vec(),
            executable: false,
        };
        let first = archive("bexos-sdk", &[a.clone(), b.clone()]).unwrap();
        let second = archive("bexos-sdk", &[b, a]).unwrap();
        assert_eq!(first, second);
        assert_eq!(&first[..10], &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn rejects_unsafe_and_duplicate_paths() {
        let entry = Entry {
            path: "../bad".into(),
            bytes: vec![],
            executable: false,
        };
        assert!(archive("bexos-sdk", &[entry]).is_err());
        let entry = Entry {
            path: "same".into(),
            bytes: vec![],
            executable: false,
        };
        assert!(archive("bexos-sdk", &[entry.clone(), entry]).is_err());
    }

    #[test]
    fn supports_ustar_prefixes() {
        let path = format!("{}file.rs", "long-directory/".repeat(8));
        let entry = Entry {
            path,
            bytes: vec![],
            executable: false,
        };
        assert!(archive("bexos-sdk", &[entry]).is_ok());
    }
}
