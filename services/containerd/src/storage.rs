use crate::spec::ContainerSpec;
use bexos_fs_client::Directory;
use bexos_userspace::{Channel, Memory};
use fs_fidl::{FsStatus, NodeKind};
use std::collections::BTreeMap;

const MAX_CONFIG: usize = 64 * 1024;

fn duplicate(channel: u64) -> Result<Channel, FsStatus> {
    let (_, rights) = Memory::object_info(channel).map_err(|_| FsStatus::Io)?;
    Memory::duplicate(channel, rights)
        .map(Channel)
        .map_err(|_| FsStatus::Io)
}

fn containers(data: u64, create: bool) -> Result<Directory, FsStatus> {
    Directory::from_channel(duplicate(data)?).open_directory("containers", create)
}

pub fn load(data: u64) -> Result<BTreeMap<String, ContainerSpec>, FsStatus> {
    let root = match containers(data, false) {
        Ok(root) => root,
        Err(FsStatus::NotFound) => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    let mut result = BTreeMap::new();
    for entry in root.read_entries()? {
        if entry.kind != NodeKind::Directory || entry.name.starts_with(".staging-") {
            continue;
        }
        let bytes = root
            .open_file(&format!("{}/config.prototxt", entry.name), false, false)?
            .read_all(MAX_CONFIG)?;
        let spec = ContainerSpec::decode_prototxt(&bytes).map_err(|_| FsStatus::Corrupt)?;
        if spec.container_id != entry.name || result.insert(entry.name, spec).is_some() {
            return Err(FsStatus::Corrupt);
        }
    }
    Ok(result)
}

/// Apply a verified image into a private staging directory and publish it by
/// one directory rename only after the rootfs and durable prototxt are ready.
pub fn install(data: u64, spec: &ContainerSpec, image: &bexos_oci::Image) -> Result<(), FsStatus> {
    let root = containers(data, true)?;
    if root.open_directory(&spec.container_id, false).is_ok() {
        return Err(FsStatus::AlreadyExists);
    }
    let staging = format!(".staging-{}", spec.container_id);
    if root
        .remove_tree(&staging)
        .is_err_and(|error| error != FsStatus::NotFound)
    {
        return Err(FsStatus::Io);
    }
    let result = (|| {
        let directory = root.open_directory(&staging, true)?;
        let rootfs = directory.open_directory("rootfs", true)?;
        let mut sink = bexos_oci_fs::FilesystemSink::new(rootfs.into_channel())?;
        bexos_oci::apply_image(image, &mut sink)?;
        drop(sink);
        let bytes = spec.encode_prototxt().map_err(|_| FsStatus::InvalidArgs)?;
        let config = directory.open_file("config.prototxt", true, true)?;
        config.write_all(bytes.as_bytes())?;
        config.sync()?;
        drop(config);
        root.rename(&staging, &spec.container_id)
    })();
    if result.is_err() {
        let _ = root.remove_tree(&staging);
    }
    result
}

pub fn open_rootfs(data: u64, id: &str) -> Result<Channel, FsStatus> {
    Ok(containers(data, false)?
        .open_directory(&format!("{id}/rootfs"), false)?
        .into_channel())
}

pub fn delete(data: u64, id: &str) -> Result<(), FsStatus> {
    containers(data, false)?.remove_tree(id)
}
