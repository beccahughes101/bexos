//! The emulator host provisions fresh boot entropy at launch, never in a Bazel
//! artifact. QEMU's Cortex-A53 has no architectural RNDR instruction.
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
pub fn stage(source: &Path, destination: &Path) -> Result<(), String> {
    let mut bytes = fs::read(source).map_err(|e| format!("read boot handoff: {e}"))?;
    if bytes.len() == bexos_boot::BootHandoff::WORDS * 8
        && u64::from_le_bytes(bytes[72..80].try_into().unwrap()) <= 1
    {
        let mut seed = [0; 32];
        fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut seed))
            .map_err(|e| format!("host boot entropy unavailable: {e}"))?;
        if seed == [0; 32] {
            return Err("host boot entropy rejected".into());
        }
        bytes[72..80].copy_from_slice(&1u64.to_le_bytes());
        bytes[80..112].copy_from_slice(&seed);
        seed.fill(0);
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(destination)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|e| format!("stage private boot handoff: {e}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_private_entropy_preserves_the_rest_of_the_handoff() {
        let directory =
            std::env::temp_dir().join(format!("bexos-entropy-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source");
        let first = directory.join("first");
        let second = directory.join("second");
        let handoff = bexos_boot::BootHandoff::qemu_default(4096, 4);
        let bytes: Vec<u8> = handoff
            .words()
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        fs::write(&source, &bytes).unwrap();
        stage(&source, &first).unwrap();
        stage(&source, &second).unwrap();
        let a = fs::read(first).unwrap();
        let b = fs::read(second).unwrap();
        assert_eq!(&a[..72], &bytes[..72]);
        assert_eq!(&a[112..], &bytes[112..]);
        assert_eq!(&a[72..80], &1u64.to_le_bytes());
        assert_ne!(&a[80..112], &b[80..112]);
        assert_eq!(fs::read(source).unwrap(), bytes);
        fs::remove_dir_all(directory).unwrap();
    }
}
