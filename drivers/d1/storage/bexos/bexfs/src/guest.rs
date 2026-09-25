mod migration;
mod mount;
mod service;
mod wire;

use crate::guest_block::VolumeDevice;
use crate::{BexFs, FileHandle, NodeAttributes};
use alloc::vec::Vec;
use bexos_userspace::{Channel, Memory, Startup, log};
use fs_fidl::*;

pub(crate) struct Volume {
    fs: BexFs,
    device: VolumeDevice,
    key_handle: u64,
    owned: bool,
    managed: bool,
    prepared: Option<PreparedResize>,
}

pub(crate) struct Endpoint {
    channel: Channel,
    volume: usize,
    inode: u64,
    file: Result<Option<FileHandle>, FsStatus>,
}

pub(crate) struct MountControl {
    channel: Channel,
    volume: usize,
}

pub(crate) struct PreparedResize {
    token: u64,
    fs: BexFs,
    device: VolumeDevice,
    key_handle: u64,
    owned: bool,
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(s) => s,
            Err(error) => {
                log(&alloc::format!(
                    "bexfs: candidate migration rejected error={error:?}\n"
                ));
                bexos_userspace::exit()
            }
        }
    } else {
        log("bexfs: EL0 filesystem service ready\n");
        Startup::ready(control).unwrap();
        migration::Runtime {
            control,
            migration: start.migration,
            volumes: Vec::new(),
            endpoints: Vec::new(),
            controls: Vec::new(),
            expected_volumes: 0,
        }
    };
    service::serve(state).await
}

pub(crate) fn attributes(a: NodeAttributes) -> FileAttributes {
    FileAttributes {
        size_bytes: a.size_bytes,
        storage_allocated_bytes: a.storage_allocated_bytes,
        creation_time_nanos: a.creation_time_nanos,
        modification_time_nanos: a.modification_time_nanos,
        mode: a.mode,
        uid: a.uid,
        gid: a.gid,
    }
}
pub(crate) fn empty_attrs() -> FileAttributes {
    FileAttributes {
        size_bytes: 0,
        storage_allocated_bytes: 0,
        creation_time_nanos: 0,
        modification_time_nanos: 0,
        mode: 0,
        uid: 0,
        gid: 0,
    }
}
impl Drop for Volume {
    fn drop(&mut self) {
        if self.owned {
            let _ = Memory::close(self.key_handle);
        }
        if let Some(prepared) = self.prepared.take() {
            prepared.close();
        }
    }
}

impl PreparedResize {
    pub(crate) fn close(self) {
        if self.owned {
            let _ = Memory::close(self.key_handle);
        }
    }
}
