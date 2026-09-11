use super::{
    state::{Descriptor, DirectoryStream, Input, IoError, Output},
    wasi::filesystem::{preopens, types::*},
};
use crate::{
    context::Context,
    resources::{Kind, READ, WRITE},
};
use wasmtime::{Result, component::Resource};
pub type FsResult<T> = Result<T, ErrorCode>;
pub fn safe_path(path: &str) -> FsResult<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|p| p == ".." || p.is_empty())
    {
        return Err(ErrorCode::NotPermitted);
    }
    Ok(())
}
impl Context {
    fn descriptor(&self, r: &Resource<Descriptor>, write: bool) -> Result<FsResult<Descriptor>> {
        let d = self.wasi.table.get(r)?.clone();
        let flag = if write {
            DescriptorFlags::WRITE
        } else {
            DescriptorFlags::READ
        };
        if !d.flags.contains(flag) {
            return Ok(Err(ErrorCode::NotPermitted));
        }
        Ok(Ok(d))
    }
}
impl Host for Context {
    fn filesystem_error_code(&mut self, r: Resource<IoError>) -> Result<Option<ErrorCode>> {
        self.wasi.table.get(&r)?;
        Ok(None)
    }
}
impl preopens::Host for Context {
    fn get_directories(&mut self) -> Result<Vec<(Resource<Descriptor>, String)>> {
        let entries: Vec<_> = self
            .resources
            .entries()
            .filter(|(_, e)| e.handle.kind() == Kind::Directory)
            .map(|(_, e)| e.clone())
            .collect();
        let mut out = Vec::new();
        for entry in entries {
            let mut flags = DescriptorFlags::empty();
            if entry.handle.rights() & READ != 0 {
                flags |= DescriptorFlags::READ;
            }
            if entry.handle.rights() & WRITE != 0 {
                flags |= DescriptorFlags::WRITE | DescriptorFlags::MUTATE_DIRECTORY;
            }
            let path = entry.name.clone();
            out.push((self.push(Descriptor { entry, flags })?, path));
        }
        Ok(out)
    }
}
impl HostDirectoryEntryStream for Context {
    fn read_directory_entry(
        &mut self,
        r: Resource<DirectoryStream>,
    ) -> Result<FsResult<Option<DirectoryEntry>>> {
        Ok(Ok(self.wasi.table.get_mut(&r)?.entries.pop_front()))
    }
    fn drop(&mut self, r: Resource<DirectoryStream>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
macro_rules! unsupported {
 ($($name:ident($($arg:ident:$ty:ty),*) -> $ret:ty;)+)=>{$(fn $name(&mut self,r:Resource<Descriptor>,$($arg:$ty),*)->Result<FsResult<$ret>>{self.wasi.table.get(&r)?;let _=($($arg,)*);Ok(Err(ErrorCode::Unsupported))})+};
}
impl HostDescriptor for Context {
    fn read_via_stream(
        &mut self,
        r: Resource<Descriptor>,
        offset: u64,
    ) -> Result<FsResult<Resource<Input>>> {
        let d = match self.descriptor(&r, false)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        if d.entry.handle.kind() != Kind::File {
            return Ok(Err(ErrorCode::IsDirectory));
        }
        Ok(Ok(self.push(Input::File(d.entry, offset))?))
    }
    fn write_via_stream(
        &mut self,
        r: Resource<Descriptor>,
        offset: u64,
    ) -> Result<FsResult<Resource<Output>>> {
        let d = match self.descriptor(&r, true)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        if d.entry.handle.kind() != Kind::File {
            return Ok(Err(ErrorCode::IsDirectory));
        }
        Ok(Ok(self.push(Output::file(d.entry, offset))?))
    }
    fn append_via_stream(&mut self, r: Resource<Descriptor>) -> Result<FsResult<Resource<Output>>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = self.wasi.table.get(&r)?.clone();
        match self.host.stat_file(&*d.entry.handle) {
            Ok(s) => self.write_via_stream(r, s.size),
            Err(e) => Ok(Err(e)),
        }
    }
    fn get_flags(&mut self, r: Resource<Descriptor>) -> Result<FsResult<DescriptorFlags>> {
        Ok(Ok(self.wasi.table.get(&r)?.flags))
    }
    fn get_type(&mut self, r: Resource<Descriptor>) -> Result<FsResult<DescriptorType>> {
        Ok(Ok(
            if self.wasi.table.get(&r)?.entry.handle.kind() == Kind::Directory {
                DescriptorType::Directory
            } else {
                DescriptorType::RegularFile
            },
        ))
    }
    fn read(
        &mut self,
        r: Resource<Descriptor>,
        length: u64,
        offset: u64,
    ) -> Result<FsResult<(Vec<u8>, bool)>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = match self.descriptor(&r, false)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        if d.entry.handle.kind() != Kind::File {
            return Ok(Err(ErrorCode::IsDirectory));
        }
        let length = length.min(32768) as usize;
        Ok(self
            .host
            .read_file(&*d.entry.handle, offset, length)
            .map(|b| {
                let eof = b.is_empty() && length > 0;
                (b, eof)
            })
            .map_err(|_| ErrorCode::Io))
    }
    fn write(
        &mut self,
        r: Resource<Descriptor>,
        bytes: Vec<u8>,
        offset: u64,
    ) -> Result<FsResult<u64>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = match self.descriptor(&r, true)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        if d.entry.handle.kind() != Kind::File {
            return Ok(Err(ErrorCode::IsDirectory));
        }
        if bytes.len() > 32768 {
            return Ok(Err(ErrorCode::InsufficientMemory));
        }
        Ok(self
            .host
            .write_file(&*d.entry.handle, offset, &bytes)
            .map(|n| n as u64)
            .map_err(|_| ErrorCode::Io))
    }
    fn sync_data(&mut self, r: Resource<Descriptor>) -> Result<FsResult<()>> {
        self.sync(r)
    }
    fn sync(&mut self, r: Resource<Descriptor>) -> Result<FsResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = self.wasi.table.get(&r)?;
        Ok(self.host.sync_file(&*d.entry.handle))
    }
    fn set_size(&mut self, r: Resource<Descriptor>, size: u64) -> Result<FsResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = match self.descriptor(&r, true)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        Ok(self.host.resize_file(&*d.entry.handle, size))
    }
    fn stat(&mut self, r: Resource<Descriptor>) -> Result<FsResult<DescriptorStat>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = self.wasi.table.get(&r)?;
        Ok(self.host.stat_file(&*d.entry.handle))
    }
    fn read_directory(
        &mut self,
        r: Resource<Descriptor>,
    ) -> Result<FsResult<Resource<DirectoryStream>>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let d = match self.descriptor(&r, false)? {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        if d.entry.handle.kind() != Kind::Directory {
            return Ok(Err(ErrorCode::NotDirectory));
        }
        match self.host.read_directory(&*d.entry.handle) {
            Ok(entries) => Ok(Ok(self.push(DirectoryStream {
                entries: entries.into(),
            })?)),
            Err(e) => Ok(Err(e)),
        }
    }
    fn open_at(
        &mut self,
        r: Resource<Descriptor>,
        _path_flags: PathFlags,
        path: String,
        open: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<FsResult<Resource<Descriptor>>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        if let Err(e) = safe_path(&path) {
            return Ok(Err(e));
        }
        let d = self.wasi.table.get(&r)?.clone();
        if d.entry.handle.kind() != Kind::Directory {
            return Ok(Err(ErrorCode::NotDirectory));
        }
        let writable = flags.intersects(DescriptorFlags::WRITE | DescriptorFlags::MUTATE_DIRECTORY)
            || open.intersects(OpenFlags::CREATE | OpenFlags::TRUNCATE);
        if writable && !d.flags.contains(DescriptorFlags::MUTATE_DIRECTORY) {
            return Ok(Err(ErrorCode::ReadOnly));
        }
        if flags.contains(DescriptorFlags::READ) && !d.flags.contains(DescriptorFlags::READ) {
            return Ok(Err(ErrorCode::NotPermitted));
        }
        if flags.intersects(
            DescriptorFlags::REQUESTED_WRITE_SYNC
                | DescriptorFlags::DATA_INTEGRITY_SYNC
                | DescriptorFlags::FILE_INTEGRITY_SYNC,
        ) {
            return Ok(Err(ErrorCode::Unsupported));
        }
        let mut reservation = self.reserve_resources(1)?;
        match self.host.open_file(&*d.entry.handle, &path, open, flags) {
            Ok(entry) => Ok(Ok(
                self.push_reserved(Descriptor { entry, flags }, &mut reservation)?
            )),
            Err(e) => Ok(Err(e)),
        }
    }
    fn create_directory_at(
        &mut self,
        r: Resource<Descriptor>,
        path: String,
    ) -> Result<FsResult<()>> {
        let result = self.open_at(
            r,
            PathFlags::empty(),
            path,
            OpenFlags::CREATE | OpenFlags::DIRECTORY,
            DescriptorFlags::READ | DescriptorFlags::WRITE | DescriptorFlags::MUTATE_DIRECTORY,
        )?;
        match result {
            Ok(r) => {
                self.delete(r)?;
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn stat_at(
        &mut self,
        r: Resource<Descriptor>,
        pf: PathFlags,
        path: String,
    ) -> Result<FsResult<DescriptorStat>> {
        match self.open_at(r, pf, path, OpenFlags::empty(), DescriptorFlags::READ)? {
            Ok(r) => {
                let result = self.stat(Resource::new_borrow(r.rep()));
                self.delete(r)?;
                result
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn unlink_file_at(&mut self, r: Resource<Descriptor>, path: String) -> Result<FsResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        if let Err(e) = safe_path(&path) {
            return Ok(Err(e));
        }
        let d = self.wasi.table.get(&r)?;
        if !d.flags.contains(DescriptorFlags::MUTATE_DIRECTORY) {
            return Ok(Err(ErrorCode::ReadOnly));
        }
        Ok(self.host.unlink_file(&*d.entry.handle, &path, false))
    }
    fn remove_directory_at(
        &mut self,
        r: Resource<Descriptor>,
        path: String,
    ) -> Result<FsResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        if let Err(e) = safe_path(&path) {
            return Ok(Err(e));
        }
        let d = self.wasi.table.get(&r)?;
        if !d.flags.contains(DescriptorFlags::MUTATE_DIRECTORY) {
            return Ok(Err(ErrorCode::ReadOnly));
        }
        Ok(self.host.unlink_file(&*d.entry.handle, &path, true))
    }
    fn is_same_object(
        &mut self,
        r: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> Result<bool> {
        Ok(std::sync::Arc::ptr_eq(
            &self.wasi.table.get(&r)?.entry.handle,
            &self.wasi.table.get(&other)?.entry.handle,
        ))
    }
    unsupported! {
     advise(offset:u64,length:u64,advice:Advice)->();
     set_times(access:NewTimestamp,modification:NewTimestamp)->();
     set_times_at(flags:PathFlags,path:String,access:NewTimestamp,modification:NewTimestamp)->();
     link_at(flags:PathFlags,old:String,new:Resource<Descriptor>,path:String)->();
     readlink_at(path:String)->String;
     rename_at(old:String,new:Resource<Descriptor>,path:String)->();
     symlink_at(old:String,new:String)->();
     metadata_hash()->MetadataHashValue;
     metadata_hash_at(flags:PathFlags,path:String)->MetadataHashValue;
    }
    fn drop(&mut self, r: Resource<Descriptor>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_preopen() {
        for p in [
            "../secret",
            "a/../secret",
            "/data/secret",
            "a//b",
            "a\0b",
            "",
        ] {
            assert!(safe_path(p).is_err());
        }
        for p in ["file", "sub/file", "."] {
            assert!(safe_path(p).is_ok());
        }
    }
}
