use super::*;
use bexos_userspace::fs as native;
fn error(e: fs_fidl::FsStatus) -> fs::ErrorCode {
    use fs_fidl::FsStatus::*;
    match e {
        NotFound => fs::ErrorCode::NoEntry,
        AccessDenied => fs::ErrorCode::NotPermitted,
        AlreadyExists => fs::ErrorCode::Exist,
        InvalidArgs => fs::ErrorCode::Invalid,
        NotDirectory => fs::ErrorCode::NotDirectory,
        NoSpace => fs::ErrorCode::InsufficientSpace,
        _ => fs::ErrorCode::Io,
    }
}
pub fn open(
    dir: &dyn Handle,
    path: &str,
    open: fs::OpenFlags,
    flags: fs::DescriptorFlags,
) -> FsResult<Entry> {
    bexos_wasm_runtime::wasi::filesystem::safe_path(path)?;
    if open.contains(fs::OpenFlags::EXCLUSIVE) {
        return Err(fs::ErrorCode::Unsupported);
    }
    let read = flags.contains(fs::DescriptorFlags::READ);
    let write =
        flags.intersects(fs::DescriptorFlags::WRITE | fs::DescriptorFlags::MUTATE_DIRECTORY);
    let native_flags = if read { 1 } else { 0 }
        | if write { 2 } else { 0 }
        | if open.contains(fs::OpenFlags::CREATE) {
            8
        } else {
            0
        }
        | if open.contains(fs::OpenFlags::TRUNCATE) {
            16
        } else {
            0
        }
        | if open.contains(fs::OpenFlags::DIRECTORY) {
            32
        } else {
            0
        };
    let channel = native::open(Channel(dir.native()), path, native_flags).map_err(error)?;
    let attr = match native::attributes(channel) {
        Ok(a) => a,
        Err(e) => {
            let _ = native::close(channel);
            return Err(error(e));
        }
    };
    let kind = if attr.mode & 0o170000 == 0o040000 || open.contains(fs::OpenFlags::DIRECTORY) {
        Kind::Directory
    } else {
        Kind::File
    };
    Ok(entry(
        "",
        channel.0,
        kind,
        TRANSFER | if read { READ } else { 0 } | if write { WRITE } else { 0 },
    ))
}
pub fn read(h: &dyn Handle, offset: u64, length: usize) -> Result<Vec<u8>> {
    let offset = i64::try_from(offset)?;
    native::seek(Channel(h.native()), offset).map_err(|e| wasmtime::format_err!("seek: {e:?}"))?;
    native::read(Channel(h.native()), length as u64)
        .map_err(|e| wasmtime::format_err!("read: {e:?}"))
}
pub fn write(h: &dyn Handle, offset: u64, bytes: &[u8]) -> Result<usize> {
    let offset = i64::try_from(offset)?;
    native::seek(Channel(h.native()), offset).map_err(|e| wasmtime::format_err!("seek: {e:?}"))?;
    native::write(Channel(h.native()), bytes).map_err(|e| wasmtime::format_err!("write: {e:?}"))?;
    Ok(bytes.len())
}
pub fn stat(h: &dyn Handle) -> FsResult<fs::DescriptorStat> {
    let a = native::attributes(Channel(h.native())).map_err(error)?;
    Ok(fs::DescriptorStat {
        type_: if h.kind() == Kind::Directory {
            fs::DescriptorType::Directory
        } else {
            fs::DescriptorType::RegularFile
        },
        link_count: 1,
        size: a.size_bytes,
        data_access_timestamp: None,
        data_modification_timestamp: Some(
            bexos_wasm_runtime::wasi::wasi::clocks::wall_clock::Datetime {
                seconds: a.modification_time_nanos / 1_000_000_000,
                nanoseconds: (a.modification_time_nanos % 1_000_000_000) as u32,
            },
        ),
        status_change_timestamp: None,
    })
}
pub fn read_directory(h: &dyn Handle) -> FsResult<Vec<fs::DirectoryEntry>> {
    let entries = native::read_entries(Channel(h.native())).map_err(error)?;
    if entries.len() > 4096 {
        return Err(fs::ErrorCode::InsufficientMemory);
    }
    Ok(entries
        .into_iter()
        .map(|e| fs::DirectoryEntry {
            name: e.name,
            type_: if e.kind == fs_fidl::NodeKind::Directory {
                fs::DescriptorType::Directory
            } else {
                fs::DescriptorType::RegularFile
            },
        })
        .collect())
}
pub fn sync(h: &dyn Handle) -> FsResult<()> {
    native::sync_file(Channel(h.native())).map_err(error)
}
pub fn resize(h: &dyn Handle, size: u64) -> FsResult<()> {
    native::set_len(Channel(h.native()), size).map_err(error)
}
pub fn unlink(h: &dyn Handle, path: &str, _directory: bool) -> FsResult<()> {
    bexos_wasm_runtime::wasi::filesystem::safe_path(path)?;
    native::unlink(Channel(h.native()), path).map_err(error)
}
