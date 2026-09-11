//! Persistent root-only firmware medium. Reboot never truncates existing slots.
use std::{fs::OpenOptions, path::Path};

pub fn prepare(path: &Path) -> Result<(), String> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => {
            file.set_len(bexos_secure_firmware::store::DISK_BYTES)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("initialize firmware disk: {e}"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if !metadata.file_type().is_file()
                || metadata.len() != bexos_secure_firmware::store::DISK_BYTES
            {
                return Err(
                    "existing firmware disk has invalid geometry; refusing to replace it".into(),
                );
            }
        }
        Err(e) => return Err(format!("open firmware disk: {e}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    #[test]
    fn reboot_preserves_slots_and_rejects_damaged_geometry() {
        let path = std::env::temp_dir().join(format!("bexos-firmware-disk-{}", std::process::id()));
        prepare(&path).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.seek(SeekFrom::Start(8192)).unwrap();
        file.write_all(b"retained slot").unwrap();
        file.sync_all().unwrap();
        prepare(&path).unwrap();
        file.seek(SeekFrom::Start(8192)).unwrap();
        let mut bytes = [0; 13];
        file.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"retained slot");
        file.set_len(4096).unwrap();
        assert!(prepare(&path).is_err());
        assert_eq!(file.metadata().unwrap().len(), 4096);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
