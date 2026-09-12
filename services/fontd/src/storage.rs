use crate::{
    index::{Scope, digest},
    parser,
    runtime::{Blob, Runtime},
};
use alloc::{format, string::String};
use bexos_userspace::{Memory, fs, vfs};
use fonts_fidl::FontStatus;
use fs_fidl::{NodeKind, OpenFlags};

const FONT_DIRECTORY: &str = "fonts";

fn dir_flags(write: bool) -> u32 {
    OpenFlags::RIGHT_READABLE.0
        | OpenFlags::DIRECTORY.0
        | if write {
            OpenFlags::RIGHT_WRITABLE.0 | OpenFlags::CREATE.0
        } else {
            0
        }
}

fn file_flags(write: bool) -> u32 {
    OpenFlags::RIGHT_READABLE.0
        | if write {
            OpenFlags::RIGHT_WRITABLE.0 | OpenFlags::CREATE.0 | OpenFlags::TRUNCATE.0
        } else {
            0
        }
}

pub fn load_user(runtime: &mut Runtime, uid: u64) -> Result<(), FontStatus> {
    if runtime.loaded_users.contains(&uid) {
        return Ok(());
    }
    runtime.check_user(uid)?;
    let home =
        vfs::get_user_home_directory(runtime.vfsd, uid).map_err(|_| FontStatus::AccessDenied)?;
    let directory = match fs::open(home, FONT_DIRECTORY, dir_flags(false)) {
        Ok(directory) => directory,
        Err(fs_fidl::FsStatus::NotFound) => {
            let _ = fs::close(home);
            runtime.loaded_users.insert(uid);
            return Ok(());
        }
        Err(_) => {
            let _ = fs::close(home);
            return Err(FontStatus::Storage);
        }
    };
    let result = (|| {
        let entries = fs::read_entries(directory).map_err(|_| FontStatus::Storage)?;
        for entry in entries {
            if entry.kind != NodeKind::File
                || entry.attr.size_bytes == 0
                || entry.attr.size_bytes > parser::MAX_FONT_BYTES as u64
            {
                continue;
            }
            let Ok(file) = fs::open(directory, &entry.name, file_flags(false)) else {
                continue;
            };
            let bytes = fs::read(file, entry.attr.size_bytes);
            let _ = fs::close(file);
            let Ok(bytes) = bytes else { continue };
            let Ok(metadata) = parser::parse(&bytes) else {
                continue;
            };
            let digest = digest(&bytes);
            if !runtime.blobs.contains_key(&digest) {
                let handle = read_only_vmo(&bytes)?;
                runtime.blobs.insert(
                    digest,
                    Blob {
                        handle,
                        len: bytes.len() as u64,
                    },
                );
            }
            runtime
                .index
                .insert(Scope::User(uid), digest, &metadata)
                .map_err(|_| FontStatus::ResourceExhausted)?;
        }
        Ok(())
    })();
    let _ = fs::close(directory);
    let _ = fs::close(home);
    result?;
    runtime.loaded_users.insert(uid);
    Ok(())
}

pub fn persist(
    runtime: &mut Runtime,
    uid: u64,
    bytes: &[u8],
    digest: &[u8; 32],
) -> Result<(), FontStatus> {
    runtime.check_user(uid)?;
    let home =
        vfs::get_user_home_directory(runtime.vfsd, uid).map_err(|_| FontStatus::AccessDenied)?;
    let directory = match fs::open(home, FONT_DIRECTORY, dir_flags(true)) {
        Ok(directory) => directory,
        Err(_) => {
            let _ = fs::close(home);
            return Err(FontStatus::Storage);
        }
    };
    let name = file_name(digest);
    if let Ok(existing) = fs::open(directory, &name, file_flags(false)) {
        let matches = fs::attributes(existing)
            .ok()
            .filter(|attributes| attributes.size_bytes == bytes.len() as u64)
            .and_then(|attributes| fs::read(existing, attributes.size_bytes).ok())
            .is_some_and(|stored| crate::index::digest(&stored) == *digest);
        let _ = fs::close(existing);
        if matches {
            let _ = fs::close(directory);
            let _ = fs::close(home);
            return Ok(());
        }
        // A prior interrupted write must never make a later idempotent install
        // appear successful. Remove it before recreating the digest-named file.
        if fs::unlink(directory, &name).is_err() {
            let _ = fs::close(directory);
            let _ = fs::close(home);
            return Err(FontStatus::Storage);
        }
    }
    let result = (|| {
        let file = fs::open(directory, &name, file_flags(true)).map_err(|_| FontStatus::Storage)?;
        if let Err(status) = fs::write(file, bytes).and_then(|()| fs::sync_file(file)) {
            let _ = fs::close(file);
            let _ = fs::unlink(directory, &name);
            return Err(match status {
                fs_fidl::FsStatus::NoSpace => FontStatus::ResourceExhausted,
                _ => FontStatus::Storage,
            });
        }
        if fs::close(file).is_err() {
            let _ = fs::unlink(directory, &name);
            return Err(FontStatus::Storage);
        }
        Ok(())
    })();
    let _ = fs::close(directory);
    let _ = fs::close(home);
    result
}

pub fn read_only_vmo(bytes: &[u8]) -> Result<u64, FontStatus> {
    let raw = Memory::from_bytes(bytes).map_err(|_| FontStatus::ResourceExhausted)?;
    let duplicate =
        Memory::duplicate(raw, 1 | 2 | 16 | 32).map_err(|_| FontStatus::ResourceExhausted);
    let _ = Memory::close(raw);
    duplicate
}

fn file_name(digest: &[u8; 32]) -> String {
    let mut value = String::with_capacity(69);
    for byte in digest {
        value.push_str(&format!("{byte:02x}"));
    }
    value.push_str(".sfnt");
    value
}
