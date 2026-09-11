use std::env;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use bexos_bexfs::block::{BEXFS_BLOCK_SIZE, SectorPartitionDevice};
use bexos_bexfs::gpt::find_partition;
use bexos_bexfs::key::LockedVolumeKey;
use bexos_bexfs::sys_state::{SYS_STATE_V1_BYTES, SYS_STATE_V2_BYTES, Slot, SysStateV2};
use bexos_bexfs::{BexFs, FormatOptions};
use rosefs_core::block::{BlockDevice, BlockIoError};

fn main() {
    if let Err(error) = run() {
        eprintln!("bexfs_image: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--inspect") {
        inspect(&args)
    } else {
        format(&args)
    }
}

fn format(args: &[String]) -> Result<(), String> {
    let out = required(args, "--out")?;
    let size = required(args, "--size-bytes")?
        .parse::<u64>()
        .map_err(|_| "invalid --size-bytes".to_string())?;
    if size < 8 * 1024 * 1024 || !size.is_multiple_of(u64::from(BEXFS_BLOCK_SIZE)) {
        return Err("--size-bytes must be a 4096-aligned value of at least 8 MiB".to_string());
    }
    let label = required(args, "--label")?;
    let volume_uuid = parse_uuid(&required(args, "--volume-uuid")?)?;
    let key = read_key(&required(args, "--key-file")?)?;
    let path = PathBuf::from(out);
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.set_len(size)
        .map_err(|error| format!("size image: {error}"))?;
    let mut device = FileDevice::new(file, BEXFS_BLOCK_SIZE)?;
    let mut fs = BexFs::format(
        &mut device,
        LockedVolumeKey::new(&key).map_err(|_| "invalid key".to_string())?,
        FormatOptions {
            label: &label,
            volume_uuid,
            device_uuid: derive_device_uuid(volume_uuid),
        },
    )
    .map_err(|error| format!("format failed: {error:?}"))?;

    if args.iter().any(|arg| arg == "--initial-sys-state") {
        write_entry(&mut fs, "boot_state.bin", &SysStateV2::initial().encode())?;
    }
    for entry in repeated(args, "--entry") {
        let (destination, source) = entry
            .split_once('=')
            .ok_or_else(|| "--entry requires destination=source".to_string())?;
        let destination = destination.strip_prefix('/').unwrap_or(destination);
        let bytes = fs::read(source).map_err(|error| format!("read {source}: {error}"))?;
        write_entry(&mut fs, destination, &bytes)?;
    }
    fs.close(&mut device)
        .map_err(|error| format!("sync failed: {error:?}"))?;
    Ok(())
}

fn inspect(args: &[String]) -> Result<(), String> {
    let image = required(args, "--image")?;
    let label = required(args, "--label")?;
    let key = read_key(&required(args, "--key-file")?)?;
    let path = optional(args, "--path");
    let entries = repeated(args, "--entry");
    let update = optional(args, "--set-generation").is_some() || !entries.is_empty();
    let file = OpenOptions::new()
        .read(true)
        .write(update)
        .open(&image)
        .map_err(|error| format!("open {image}: {error}"))?;
    if let Some(partition_label) = optional(args, "--partition") {
        let disk = FileDevice::new(file, 512)?;
        let partition = find_partition(&disk, &partition_label)
            .map_err(|error| format!("GPT lookup failed: {error:?}"))?;
        let mut partition_device =
            SectorPartitionDevice::new(&disk, partition.first_lba, partition.sector_count())
                .map_err(|error| format!("partition geometry: {error:?}"))?;
        inspect_device(&mut partition_device, &key, &label, path, args)
    } else {
        let mut device = FileDevice::new(file, BEXFS_BLOCK_SIZE)?;
        inspect_device(&mut device, &key, &label, path, args)
    }
}

fn inspect_device(
    device: &mut dyn BlockDevice,
    key: &[u8; 32],
    label: &str,
    path: Option<String>,
    args: &[String],
) -> Result<(), String> {
    let update_generation = optional(args, "--set-generation")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "bad --set-generation".to_string())
        })
        .transpose()?;
    let update = update_generation.is_some() || !repeated(args, "--entry").is_empty();
    let mut fs = BexFs::mount(
        device,
        LockedVolumeKey::new(key).map_err(|_| "invalid key".to_string())?,
        label,
        !update,
    )
    .map_err(|error| format!("mount failed: {error:?}"))?;
    if let Some(path) = path {
        let mut handle = fs
            .open(
                fs.root_inode(),
                path.strip_prefix('/').unwrap_or(&path),
                if update_generation.is_some() {
                    0x1 | 0x2
                } else {
                    0x1
                },
            )
            .map_err(|error| format!("open failed: {error:?}"))?;
        let bytes = fs
            .read(&mut handle, 16 * 1024 * 1024)
            .map_err(|error| format!("read failed: {error:?}"))?;
        if args.iter().any(|arg| arg == "--sys-state") {
            if bytes.len() != SYS_STATE_V1_BYTES && bytes.len() != SYS_STATE_V2_BYTES {
                return Err("SYS_STATE record has the wrong length".to_string());
            }
            let mut state =
                SysStateV2::decode_any(&bytes).map_err(|error| format!("decode: {error:?}"))?;
            if let Some(generation) = update_generation {
                state.generation = generation;
                state.sequence = state.sequence.saturating_add(1);
                if let Some(slot) = optional(args, "--set-active-slot") {
                    state.active_slot = match slot.as_str() {
                        "A" | "a" => Slot::A,
                        "B" | "b" => Slot::B,
                        _ => return Err("--set-active-slot must be A or B".to_string()),
                    };
                }
                fs.seek(&mut handle, 0, 0)
                    .map_err(|error| format!("seek failed: {error:?}"))?;
                fs.write(&mut handle, &state.encode())
                    .map_err(|error| format!("write failed: {error:?}"))?;
            }
            if let Some(expected) = optional(args, "--expect-generation") {
                let expected = expected
                    .parse::<u64>()
                    .map_err(|_| "bad generation".to_string())?;
                if state.generation != expected {
                    return Err(format!("generation {} != {expected}", state.generation));
                }
            }
            println!(
                "bexfs: sys_state generation={} active_slot={:?} lkg_slot={:?} lkg_version={}",
                state.generation,
                state.active_slot,
                state.last_known_good_slot,
                state.last_known_good_version
            );
        } else {
            println!("bexfs: inspected {} bytes at {path}", bytes.len());
            if let Some(expected) = optional(args, "--expect-text") {
                if bytes != expected.as_bytes() {
                    return Err("file contents differ from expected text".into());
                }
            }
        }
    }
    for entry in repeated(args, "--entry") {
        let (destination, source) = entry
            .split_once('=')
            .ok_or_else(|| "--entry requires destination=source".to_string())?;
        let destination = destination.strip_prefix('/').unwrap_or(destination);
        let bytes = fs::read(source).map_err(|error| format!("read {source}: {error}"))?;
        write_entry(&mut fs, destination, &bytes)?;
    }
    if update {
        fs.sync(device)
            .map_err(|error| format!("sync failed: {error:?}"))?;
    }
    fs.close(device)
        .map_err(|error| format!("close failed: {error:?}"))?;
    Ok(())
}

fn write_entry(fs: &mut BexFs, path: &str, bytes: &[u8]) -> Result<(), String> {
    let parts: Vec<_> = path.split('/').collect();
    let mut parent = fs.root_inode();
    for component in &parts[..parts.len() - 1] {
        parent = fs
            .open(parent, component, 0x1 | 0x2 | 0x8 | 0x20)
            .map_err(|error| format!("create directory {component}: {error:?}"))?
            .inode();
    }
    let mut handle = fs
        .open(parent, parts.last().unwrap(), 0x1 | 0x2 | 0x8 | 0x10)
        .map_err(|error| format!("create {path}: {error:?}"))?;
    fs.write(&mut handle, bytes)
        .map_err(|error| format!("write {path}: {error:?}"))?;
    Ok(())
}

struct FileDevice {
    file: File,
    block_size: u32,
    blocks: u64,
}

impl FileDevice {
    fn new(file: File, block_size: u32) -> Result<Self, String> {
        let len = file.metadata().map_err(|error| error.to_string())?.len();
        if len == 0 || !len.is_multiple_of(u64::from(block_size)) {
            return Err("image size is not block aligned".to_string());
        }
        Ok(Self {
            file,
            block_size,
            blocks: len / u64::from(block_size),
        })
    }
}

impl BlockDevice for FileDevice {
    fn num_blocks(&self) -> u64 {
        self.blocks
    }
    fn block_size(&self) -> u32 {
        self.block_size
    }
    fn read_at(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        let read = self
            .file
            .read_at(buf, lba * u64::from(self.block_size))
            .map_err(|_| BlockIoError::Read)?;
        if read == buf.len() {
            Ok(())
        } else {
            Err(BlockIoError::Read)
        }
    }
    fn write_at(&self, lba: u64, buf: &[u8]) -> Result<(), BlockIoError> {
        self.check_request(lba, buf.len())?;
        let written = self
            .file
            .write_at(buf, lba * u64::from(self.block_size))
            .map_err(|_| BlockIoError::Write)?;
        if written == buf.len() {
            Ok(())
        } else {
            Err(BlockIoError::Write)
        }
    }
    fn flush(&self) -> Result<(), BlockIoError> {
        self.file.sync_all().map_err(|_| BlockIoError::Flush)
    }
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    optional(args, name).ok_or_else(|| format!("missing {name}"))
}
fn optional(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
fn repeated<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .collect()
}

fn read_key(path: &str) -> Result<[u8; 32], String> {
    let raw = fs::read(Path::new(path)).map_err(|error| format!("read key: {error}"))?;
    if raw.len() == 32 {
        return raw.try_into().map_err(|_| "invalid key".to_string());
    }
    let text = std::str::from_utf8(&raw)
        .map_err(|_| "key must be raw or hex".to_string())?
        .trim();
    if text.len() != 64 {
        return Err("hex key must contain 64 characters".to_string());
    }
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid hex key".to_string())?;
    }
    Ok(key)
}

fn parse_uuid(value: &str) -> Result<[u8; 16], String> {
    let compact = value.chars().filter(|ch| *ch != '-').collect::<String>();
    if compact.len() != 32 {
        return Err("UUID must contain 32 hexadecimal digits".to_string());
    }
    let mut uuid = [0u8; 16];
    for (index, byte) in uuid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid UUID".to_string())?;
    }
    Ok(uuid)
}

fn derive_device_uuid(mut uuid: [u8; 16]) -> [u8; 16] {
    for byte in &mut uuid {
        *byte ^= 0xa5;
    }
    uuid
}
