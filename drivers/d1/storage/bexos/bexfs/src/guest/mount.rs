use alloc::rc::Rc;
use bexos_userspace::{Channel, Memory};
use fs_fidl::FsStatus;

use crate::BexFs;
use crate::block::FidlBlockDevice;
use crate::gpt::find_partition;
use crate::guest::{PreparedResize, Volume, migration};
use crate::guest_block::{Partition, RpcBlock, SharedBlock, VolumeDevice};
use crate::key::LockedVolumeKey;
use crate::server::status;
use rosefs_core::block::BlockDevice;

pub(crate) fn mount(
    block: u64,
    label: &str,
    key: u64,
    read_only: bool,
) -> Result<Volume, FsStatus> {
    bexos_userspace::log("bexfs: mount map key\n");
    let va = Memory::map(key, 4096, 2).map_err(|_| FsStatus::AccessDenied)?;
    let locked = LockedVolumeKey::new(unsafe { core::slice::from_raw_parts(va as *const u8, 32) })
        .map_err(|_| FsStatus::Locked);
    Memory::unmap(va, 4096).map_err(|_| FsStatus::Io)?;
    Memory::close(key).map_err(|_| FsStatus::Io)?;
    bexos_userspace::log("bexfs: mount connect block\n");
    let disk = SharedBlock(Rc::new(
        FidlBlockDevice::connect(RpcBlock::connect(Channel(block))).map_err(|_| FsStatus::Io)?,
    ));
    bexos_userspace::log("bexfs: mount find partition\n");
    let part = find_partition(&disk, label).map_err(|_| FsStatus::Corrupt)?;
    if part.first_lba % 8 != 0 || part.sector_count() % 8 != 0 {
        return Err(FsStatus::Corrupt);
    }
    let mut device = Partition {
        disk,
        first: part.first_lba,
        sectors: part.sector_count(),
    };
    bexos_userspace::log("bexfs: mount open fs\n");
    let fs = BexFs::mount(&mut device, locked?, label, read_only).map_err(status)?;
    bexos_userspace::log("bexfs: mount key handle\n");
    let key_handle = Memory::from_bytes(fs.checkpoint_key()).map_err(|_| FsStatus::NoSpace)?;
    bexos_userspace::log("bexfs: mount complete\n");
    Ok(Volume {
        fs,
        device: VolumeDevice::Partition(device),
        key_handle,
        owned: true,
        managed: false,
        prepared: None,
    })
}

pub(crate) fn raw_mount(
    block: u64,
    label: &str,
    key: u64,
    read_only: bool,
    format: bool,
) -> Result<Volume, FsStatus> {
    let (locked, key_handle) = read_volume_key(key)?;
    let disk = SharedBlock(Rc::new(
        FidlBlockDevice::connect(RpcBlock::connect(Channel(block))).map_err(|_| FsStatus::Io)?,
    ));
    if disk.block_size() != 4096 {
        return Err(FsStatus::InvalidArgs);
    }
    let mut device = VolumeDevice::Raw4k(disk);
    let fs = if format {
        BexFs::format(
            &mut device,
            locked,
            crate::FormatOptions {
                label,
                volume_uuid: uuid_from_label(label, 1),
                device_uuid: uuid_from_label(label, 2),
            },
        )
    } else {
        BexFs::mount(&mut device, locked, label, read_only)
    }
    .map_err(status)?;
    Ok(Volume {
        fs,
        device,
        key_handle,
        owned: true,
        managed: true,
        prepared: None,
    })
}

pub(crate) fn prepare_resize(
    current: &mut Volume,
    block: u64,
    label: &str,
    key: u64,
) -> Result<PreparedResize, FsStatus> {
    let (locked, key_handle) = read_volume_key(key)?;
    let disk = SharedBlock(Rc::new(
        FidlBlockDevice::connect(RpcBlock::connect(Channel(block))).map_err(|_| FsStatus::Io)?,
    ));
    if disk.block_size() != 4096 {
        return Err(FsStatus::InvalidArgs);
    }
    let mut next_device = VolumeDevice::Raw4k(disk);
    let next = current
        .fs
        .copy_to_new_device(
            &mut current.device,
            &mut next_device,
            label,
            uuid_from_label(label, 1),
            uuid_from_label(label, 2),
        )
        .map_err(status)?;
    drop(locked);
    Ok(PreparedResize {
        token: current.fs.generation().saturating_add(1),
        fs: next,
        device: next_device,
        key_handle,
        owned: true,
    })
}

fn read_volume_key(key: u64) -> Result<(LockedVolumeKey, u64), FsStatus> {
    let va = Memory::map(key, 4096, 2).map_err(|_| FsStatus::AccessDenied)?;
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(unsafe { core::slice::from_raw_parts(va as *const u8, 32) });
    let locked = LockedVolumeKey::new(&bytes).map_err(|_| FsStatus::Locked)?;
    bytes.fill(0);
    Memory::unmap(va, 4096).map_err(|_| FsStatus::Io)?;
    // Preserve the read-only key capability for handover without requesting
    // execute permission that the caller never granted.
    let key_handle = Memory::duplicate(key, 1 | 2 | 16 | 32).map_err(|_| FsStatus::Io)?;
    Memory::close(key).map_err(|_| FsStatus::Io)?;
    Ok((locked, key_handle))
}

fn uuid_from_label(label: &str, salt: u8) -> [u8; 16] {
    let mut out = [salt; 16];
    for (i, byte) in label.as_bytes().iter().take(16).enumerate() {
        out[i] ^= *byte;
    }
    out
}

pub(crate) fn endpoint_key(channel: Channel) -> u64 {
    migration::ENDPOINT | channel.0
}
