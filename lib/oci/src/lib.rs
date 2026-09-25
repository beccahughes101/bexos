//! Strict, offline OCI image-layout validation and layer planning.

mod gzip;

use ruzstd::decoding::StreamingDecoder;
use ruzstd::io_nostd::Read as RuzstdRead;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const BLOCK: usize = 512;
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_LAYER_BYTES: usize = 256 * 1024 * 1024;
const MAX_IMAGE_UNPACKED_BYTES: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 262_144;
const MAX_XATTRS: usize = 256;
const MAX_XATTR_VALUE_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidLayout,
    InvalidJson,
    InvalidDigest,
    InvalidTar,
    UnsafePath,
    UnsupportedMediaType,
    UnsupportedPlatform,
    ResourceExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessConfig {
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub working_directory: String,
    pub user: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerOperation {
    Directory {
        path: String,
        mode: u32,
        uid: u32,
        gid: u32,
        mtime: u64,
    },
    File {
        path: String,
        data: Vec<u8>,
        mode: u32,
        uid: u32,
        gid: u32,
        mtime: u64,
    },
    Symlink {
        path: String,
        target: String,
        uid: u32,
        gid: u32,
    },
    Hardlink {
        path: String,
        target: String,
    },
    Remove {
        path: String,
    },
    OpaqueDirectory {
        path: String,
    },
    Xattrs {
        path: String,
        values: Vec<(String, Vec<u8>)>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub process: ProcessConfig,
    pub layers: Vec<Vec<LayerOperation>>,
    pub manifest_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
}

/// A filesystem target for a validated OCI layer plan. Implementations must
/// keep all paths rooted in the container root passed to them and must make
/// replacement operations atomic with respect to readers.
pub trait LayerSink {
    type Error;

    fn ensure_directory(&mut self, path: &str, metadata: Metadata) -> Result<(), Self::Error>;
    fn replace_file(
        &mut self,
        path: &str,
        data: &[u8],
        metadata: Metadata,
    ) -> Result<(), Self::Error>;
    fn replace_symlink(
        &mut self,
        path: &str,
        target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), Self::Error>;
    fn replace_hardlink(&mut self, path: &str, target: &str) -> Result<(), Self::Error>;
    fn remove(&mut self, path: &str) -> Result<(), Self::Error>;
    fn remove_children(&mut self, path: &str) -> Result<(), Self::Error>;
    fn set_xattrs(&mut self, path: &str, values: &[(String, Vec<u8>)]) -> Result<(), Self::Error>;
}

pub fn apply_image<S: LayerSink>(image: &Image, sink: &mut S) -> Result<(), S::Error> {
    for layer in &image.layers {
        apply_layer(layer, sink)?;
    }
    Ok(())
}

pub fn apply_layer<S: LayerSink>(
    operations: &[LayerOperation],
    sink: &mut S,
) -> Result<(), S::Error> {
    for operation in operations {
        match operation {
            LayerOperation::Directory {
                path,
                mode,
                uid,
                gid,
                mtime,
            } => sink.ensure_directory(
                path,
                Metadata {
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                    mtime: *mtime,
                },
            )?,
            LayerOperation::File {
                path,
                data,
                mode,
                uid,
                gid,
                mtime,
            } => sink.replace_file(
                path,
                data,
                Metadata {
                    mode: *mode,
                    uid: *uid,
                    gid: *gid,
                    mtime: *mtime,
                },
            )?,
            LayerOperation::Symlink {
                path,
                target,
                uid,
                gid,
            } => sink.replace_symlink(path, target, *uid, *gid)?,
            LayerOperation::Hardlink { path, target } => sink.replace_hardlink(path, target)?,
            LayerOperation::Remove { path } => sink.remove(path)?,
            LayerOperation::OpaqueDirectory { path } => sink.remove_children(path)?,
            LayerOperation::Xattrs { path, values } => sink.set_xattrs(path, values)?,
        }
    }
    Ok(())
}

pub fn parse_layout(bytes: &[u8], architecture: &str) -> Result<Image, Error> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(Error::ResourceExhausted);
    }
    let entries = tar_entries(bytes)?;
    let mut files = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry.kind == TarKind::File) {
        if files.insert(entry.path.as_str(), entry.data).is_some() {
            return Err(Error::InvalidLayout);
        }
    }
    let layout = json(required(&files, "oci-layout")?)?;
    if layout.get("imageLayoutVersion").and_then(Value::as_str) != Some("1.0.0") {
        return Err(Error::InvalidLayout);
    }
    let index = json(required(&files, "index.json")?)?;
    if index.get("schemaVersion").and_then(Value::as_u64) != Some(2) {
        return Err(Error::InvalidLayout);
    }
    let descriptor = index
        .get("manifests")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidLayout)?
        .iter()
        .find(|descriptor| {
            descriptor.get("platform").is_some_and(|platform| {
                platform.get("os").and_then(Value::as_str) == Some("linux")
                    && platform.get("architecture").and_then(Value::as_str) == Some(architecture)
            })
        })
        .ok_or(Error::UnsupportedPlatform)?;
    require_media(descriptor, &["application/vnd.oci.image.manifest.v1+json"])?;
    let (manifest_digest, manifest) = descriptor_blob(&files, descriptor)?;
    let manifest_json = json(manifest)?;
    if manifest_json.get("schemaVersion").and_then(Value::as_u64) != Some(2)
        || manifest_json.get("mediaType").and_then(Value::as_str)
            != Some("application/vnd.oci.image.manifest.v1+json")
    {
        return Err(Error::InvalidLayout);
    }
    let config_descriptor = manifest_json.get("config").ok_or(Error::InvalidLayout)?;
    require_media(
        config_descriptor,
        &["application/vnd.oci.image.config.v1+json"],
    )?;
    let (_, config) = descriptor_blob(&files, config_descriptor)?;
    let config = json(config)?;
    if config.get("os").and_then(Value::as_str) != Some("linux")
        || config.get("architecture").and_then(Value::as_str) != Some(architecture)
        || config.pointer("/rootfs/type").and_then(Value::as_str) != Some("layers")
    {
        return Err(Error::UnsupportedPlatform);
    }
    let diff_ids = config
        .pointer("/rootfs/diff_ids")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidLayout)?;
    let layer_descriptors = manifest_json
        .get("layers")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidLayout)?;
    if diff_ids.len() != layer_descriptors.len() || layer_descriptors.len() > 128 {
        return Err(Error::InvalidLayout);
    }
    let mut layers = Vec::with_capacity(layer_descriptors.len());
    let mut unpacked_bytes = 0usize;
    let mut unpacked_entries = 0usize;
    for (descriptor, diff_id) in layer_descriptors.iter().zip(diff_ids) {
        let media = descriptor
            .get("mediaType")
            .and_then(Value::as_str)
            .ok_or(Error::InvalidLayout)?;
        let (_, compressed) = descriptor_blob(&files, descriptor)?;
        let uncompressed = match media {
            "application/vnd.oci.image.layer.v1.tar" => compressed.to_vec(),
            "application/vnd.oci.image.layer.v1.tar+zstd" => decompress_zstd(compressed)?,
            "application/vnd.oci.image.layer.v1.tar+gzip" => {
                gzip::decompress(compressed, MAX_LAYER_BYTES)?
            }
            _ => return Err(Error::UnsupportedMediaType),
        };
        if uncompressed.len() > MAX_LAYER_BYTES
            || parse_digest(diff_id.as_str().ok_or(Error::InvalidDigest)?)?
                != <[u8; 32]>::from(Sha256::digest(&uncompressed))
        {
            return Err(Error::InvalidDigest);
        }
        unpacked_bytes = unpacked_bytes
            .checked_add(uncompressed.len())
            .filter(|total| *total <= MAX_IMAGE_UNPACKED_BYTES)
            .ok_or(Error::ResourceExhausted)?;
        let plan = layer_plan(&uncompressed)?;
        unpacked_entries = unpacked_entries
            .checked_add(plan.len())
            .filter(|total| *total <= MAX_ENTRIES)
            .ok_or(Error::ResourceExhausted)?;
        layers.push(plan);
    }
    Ok(Image {
        process: process_config(&config)?,
        layers,
        manifest_digest,
    })
}

fn process_config(config: &Value) -> Result<ProcessConfig, Error> {
    let section = config.get("config").ok_or(Error::InvalidLayout)?;
    let mut arguments = strings(section.get("Entrypoint"))?;
    arguments.extend(strings(section.get("Cmd"))?);
    if arguments.is_empty() || arguments.len() > 256 {
        return Err(Error::InvalidLayout);
    }
    let environment = strings(section.get("Env"))?;
    if environment.len() > 256
        || environment.iter().any(|entry| {
            entry
                .split_once('=')
                .is_none_or(|(name, _)| name.is_empty())
        })
    {
        return Err(Error::InvalidLayout);
    }
    let working_directory = section
        .get("WorkingDir")
        .and_then(Value::as_str)
        .unwrap_or("/");
    if !working_directory.starts_with('/')
        || (working_directory != "/"
            && safe_path(working_directory.trim_start_matches('/')).is_err())
    {
        return Err(Error::UnsafePath);
    }
    let user = section.get("User").and_then(Value::as_str).unwrap_or("");
    if user.len() > 256 || user.contains('\0') {
        return Err(Error::InvalidLayout);
    }
    Ok(ProcessConfig {
        arguments,
        environment,
        working_directory: working_directory.into(),
        user: user.into(),
    })
}

fn strings(value: Option<&Value>) -> Result<Vec<String>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| value.len() <= 4096 && !value.contains('\0'))
                    .map(String::from)
                    .ok_or(Error::InvalidLayout)
            })
            .collect(),
        _ => Err(Error::InvalidLayout),
    }
}

fn descriptor_blob<'a>(
    files: &BTreeMap<&str, &'a [u8]>,
    descriptor: &Value,
) -> Result<([u8; 32], &'a [u8]), Error> {
    let digest = parse_digest(
        descriptor
            .get("digest")
            .and_then(Value::as_str)
            .ok_or(Error::InvalidDigest)?,
    )?;
    let size = descriptor
        .get("size")
        .and_then(Value::as_u64)
        .and_then(|size| usize::try_from(size).ok())
        .ok_or(Error::InvalidLayout)?;
    let path = format!("blobs/sha256/{}", hex(&digest));
    let bytes = required(files, &path)?;
    if bytes.len() != size || <[u8; 32]>::from(Sha256::digest(bytes)) != digest {
        return Err(Error::InvalidDigest);
    }
    Ok((digest, bytes))
}

fn require_media(value: &Value, allowed: &[&str]) -> Result<(), Error> {
    value
        .get("mediaType")
        .and_then(Value::as_str)
        .filter(|media| allowed.contains(media))
        .map(|_| ())
        .ok_or(Error::UnsupportedMediaType)
}

fn required<'a>(files: &BTreeMap<&str, &'a [u8]>, path: &str) -> Result<&'a [u8], Error> {
    files.get(path).copied().ok_or(Error::InvalidLayout)
}

fn json(bytes: &[u8]) -> Result<Value, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)
}

fn parse_digest(value: &str) -> Result<[u8; 32], Error> {
    let hex = value.strip_prefix("sha256:").ok_or(Error::InvalidDigest)?;
    if hex.len() != 64 {
        return Err(Error::InvalidDigest);
    }
    let mut digest = [0; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = (nybble(pair[0])? << 4) | nybble(pair[1])?;
    }
    Ok(digest)
}

fn nybble(value: u8) -> Result<u8, Error> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(Error::InvalidDigest),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decompress_zstd(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let decoder = StreamingDecoder::new(bytes).map_err(|_| Error::InvalidLayout)?;
    let mut output = Vec::new();
    decoder
        .take((MAX_LAYER_BYTES + 1) as u64)
        .read_to_end(&mut output)
        .map_err(|_| Error::InvalidLayout)?;
    if output.len() > MAX_LAYER_BYTES {
        return Err(Error::ResourceExhausted);
    }
    Ok(output)
}

fn layer_plan(bytes: &[u8]) -> Result<Vec<LayerOperation>, Error> {
    let entries = tar_entries(bytes)?;
    let mut operations = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = safe_path(&entry.path)?;
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        if name == ".wh..wh..opq" {
            operations.push(LayerOperation::OpaqueDirectory {
                path: parent.into(),
            });
            continue;
        }
        if let Some(removed) = name.strip_prefix(".wh.") {
            if removed.is_empty() {
                return Err(Error::UnsafePath);
            }
            operations.push(LayerOperation::Remove {
                path: if parent.is_empty() {
                    removed.into()
                } else {
                    format!("{parent}/{removed}")
                },
            });
            continue;
        }
        let operation = match entry.kind {
            TarKind::File => LayerOperation::File {
                path,
                data: entry.data.to_vec(),
                mode: entry.mode,
                uid: entry.uid,
                gid: entry.gid,
                mtime: entry.mtime,
            },
            TarKind::Directory => LayerOperation::Directory {
                path,
                mode: entry.mode,
                uid: entry.uid,
                gid: entry.gid,
                mtime: entry.mtime,
            },
            TarKind::Symlink => LayerOperation::Symlink {
                path,
                target: safe_link(&entry.link)?,
                uid: entry.uid,
                gid: entry.gid,
            },
            TarKind::Hardlink => LayerOperation::Hardlink {
                path,
                target: safe_path(&entry.link)?,
            },
        };
        operations.push(operation);
        if !entry.xattrs.is_empty() {
            operations.push(LayerOperation::Xattrs {
                path: safe_path(&entry.path)?,
                values: entry.xattrs,
            });
        }
    }
    Ok(operations)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TarKind {
    File,
    Directory,
    Symlink,
    Hardlink,
}

struct TarEntry<'a> {
    path: String,
    link: String,
    kind: TarKind,
    data: &'a [u8],
    mode: u32,
    uid: u32,
    gid: u32,
    mtime: u64,
    xattrs: Vec<(String, Vec<u8>)>,
}

#[derive(Default)]
struct PaxFields {
    path: Option<String>,
    link: Option<String>,
    xattrs: BTreeMap<String, Vec<u8>>,
}

fn tar_entries(bytes: &[u8]) -> Result<Vec<TarEntry<'_>>, Error> {
    if !bytes.len().is_multiple_of(BLOCK) {
        return Err(Error::InvalidTar);
    }
    let mut entries = Vec::new();
    let mut offset = 0usize;
    let mut pending_path: Option<String> = None;
    let mut pending_link: Option<String> = None;
    let mut pending_xattrs = BTreeMap::new();
    let mut global_xattrs = BTreeMap::new();
    let mut expanded_xattr_bytes = 0usize;
    let mut zero_blocks = 0;
    while offset < bytes.len() {
        let header = bytes.get(offset..offset + BLOCK).ok_or(Error::InvalidTar)?;
        offset += BLOCK;
        if header.iter().all(|byte| *byte == 0) {
            zero_blocks += 1;
            if zero_blocks == 2 {
                if bytes[offset..].iter().any(|byte| *byte != 0) {
                    return Err(Error::InvalidTar);
                }
                return Ok(entries);
            }
            continue;
        }
        zero_blocks = 0;
        verify_checksum(header)?;
        let size = number(&header[124..136])?;
        let size = usize::try_from(size).map_err(|_| Error::ResourceExhausted)?;
        let padded = size
            .checked_add(BLOCK - 1)
            .ok_or(Error::ResourceExhausted)?
            / BLOCK
            * BLOCK;
        let data = bytes.get(offset..offset + size).ok_or(Error::InvalidTar)?;
        offset = offset.checked_add(padded).ok_or(Error::ResourceExhausted)?;
        if offset > bytes.len() {
            return Err(Error::InvalidTar);
        }
        let typeflag = header[156];
        if matches!(typeflag, b'x' | b'g') {
            let fields = pax(data)?;
            if typeflag == b'x' {
                if let Some(path) = fields.path {
                    pending_path = Some(path);
                }
                if let Some(link) = fields.link {
                    pending_link = Some(link);
                }
                for (name, value) in fields.xattrs {
                    pending_xattrs.insert(name, value);
                }
                if pending_xattrs.len() > MAX_XATTRS {
                    return Err(Error::ResourceExhausted);
                }
            } else {
                for (name, value) in fields.xattrs {
                    global_xattrs.insert(name, value);
                }
                if global_xattrs.len() > MAX_XATTRS {
                    return Err(Error::ResourceExhausted);
                }
            }
            continue;
        }
        if typeflag == b'L' || typeflag == b'K' {
            let value = text(data)?.trim_end_matches('\0').to_string();
            if typeflag == b'L' {
                pending_path = Some(value);
            } else {
                pending_link = Some(value);
            }
            continue;
        }
        let kind = match typeflag {
            0 | b'0' => TarKind::File,
            b'5' => TarKind::Directory,
            b'2' => TarKind::Symlink,
            b'1' => TarKind::Hardlink,
            _ => return Err(Error::UnsupportedMediaType),
        };
        let prefix = field(&header[345..500])?;
        let name = field(&header[..100])?;
        let path = pending_path.take().unwrap_or_else(|| {
            if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            }
        });
        let link = pending_link.take().unwrap_or(field(&header[157..257])?);
        let mut xattrs = global_xattrs.clone();
        xattrs.append(&mut pending_xattrs);
        if xattrs.len() > MAX_XATTRS {
            return Err(Error::ResourceExhausted);
        }
        let xattr_bytes = xattrs.iter().try_fold(0usize, |total, (name, value)| {
            total.checked_add(name.len())?.checked_add(value.len())
        });
        expanded_xattr_bytes = expanded_xattr_bytes
            .checked_add(xattr_bytes.ok_or(Error::ResourceExhausted)?)
            .filter(|total| *total <= MAX_LAYER_BYTES)
            .ok_or(Error::ResourceExhausted)?;
        entries.push(TarEntry {
            path,
            link,
            kind,
            data,
            mode: u32::try_from(number(&header[100..108])?).map_err(|_| Error::InvalidTar)?,
            uid: u32::try_from(number(&header[108..116])?).map_err(|_| Error::InvalidTar)?,
            gid: u32::try_from(number(&header[116..124])?).map_err(|_| Error::InvalidTar)?,
            mtime: number(&header[136..148])?,
            xattrs: xattrs.into_iter().collect(),
        });
        if entries.len() > MAX_ENTRIES {
            return Err(Error::ResourceExhausted);
        }
    }
    Err(Error::InvalidTar)
}

fn verify_checksum(header: &[u8]) -> Result<(), Error> {
    let expected = number(&header[148..156])?;
    let actual: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                32
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    (expected == actual).then_some(()).ok_or(Error::InvalidTar)
}

fn number(bytes: &[u8]) -> Result<u64, Error> {
    if bytes.first().is_some_and(|byte| byte & 0x80 != 0) {
        return Err(Error::InvalidTar);
    }
    let value = bytes
        .split(|byte| *byte == 0 || *byte == b' ')
        .next()
        .unwrap_or(&[]);
    if value.is_empty() {
        return Ok(0);
    }
    value.iter().try_fold(0u64, |out, byte| {
        if !(b'0'..=b'7').contains(byte) {
            return Err(Error::InvalidTar);
        }
        out.checked_mul(8)
            .and_then(|out| out.checked_add(u64::from(*byte - b'0')))
            .ok_or(Error::ResourceExhausted)
    })
}

fn field(bytes: &[u8]) -> Result<String, Error> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    text(&bytes[..end]).map(String::from)
}

fn text(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|_| Error::InvalidTar)
}

fn pax(bytes: &[u8]) -> Result<PaxFields, Error> {
    let mut fields = PaxFields::default();
    let mut at = 0usize;
    while at < bytes.len() {
        let space = bytes[at..]
            .iter()
            .position(|byte| *byte == b' ')
            .map(|position| at + position)
            .ok_or(Error::InvalidTar)?;
        let length = text(&bytes[at..space])?
            .parse::<usize>()
            .map_err(|_| Error::InvalidTar)?;
        let end = at.checked_add(length).ok_or(Error::InvalidTar)?;
        let record = bytes.get(space + 1..end).ok_or(Error::InvalidTar)?;
        let record = record.strip_suffix(b"\n").ok_or(Error::InvalidTar)?;
        let separator = record
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or(Error::InvalidTar)?;
        let key = text(&record[..separator])?;
        let value = &record[separator + 1..];
        match key {
            "path" => {
                if fields.path.replace(text(value)?.into()).is_some() {
                    return Err(Error::InvalidTar);
                }
            }
            "linkpath" => {
                if fields.link.replace(text(value)?.into()).is_some() {
                    return Err(Error::InvalidTar);
                }
            }
            _ => {
                let (name, value) = if let Some(name) = key.strip_prefix("SCHILY.xattr.") {
                    validate_xattr(name, value)?;
                    (name, value.to_vec())
                } else if let Some(name) = key.strip_prefix("LIBARCHIVE.xattr.") {
                    (name, decode_base64(value)?)
                } else {
                    at = end;
                    continue;
                };
                validate_xattr(name, &value)?;
                if fields.xattrs.insert(name.into(), value).is_some()
                    || fields.xattrs.len() > MAX_XATTRS
                {
                    return Err(Error::InvalidTar);
                }
            }
        }
        at = end;
    }
    Ok(fields)
}

fn validate_xattr(name: &str, value: &[u8]) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 255
        || name.as_bytes().contains(&0)
        || !name.contains('.')
        || value.len() > MAX_XATTR_VALUE_BYTES
    {
        return Err(Error::InvalidTar);
    }
    Ok(())
}

fn decode_base64(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if bytes.len() > MAX_XATTR_VALUE_BYTES.div_ceil(3) * 4 {
        return Err(Error::ResourceExhausted);
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::InvalidTar);
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let padding = usize::from(chunk[3] == b'=') + usize::from(chunk[2] == b'=');
        if padding != 0 && !last || chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(Error::InvalidTar);
        }
        let digit = |byte| match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            b'=' => Ok(0),
            _ => Err(Error::InvalidTar),
        };
        let a = digit(chunk[0])?;
        let b = digit(chunk[1])?;
        let c = digit(chunk[2])?;
        let d = digit(chunk[3])?;
        if chunk[0] == b'='
            || chunk[1] == b'='
            || padding == 2 && b & 0x0f != 0
            || padding == 1 && c & 0x03 != 0
        {
            return Err(Error::InvalidTar);
        }
        output.push(a << 2 | b >> 4);
        if padding < 2 {
            output.push(b << 4 | c >> 2);
        }
        if padding == 0 {
            output.push(c << 6 | d);
        }
        if output.len() > MAX_XATTR_VALUE_BYTES {
            return Err(Error::ResourceExhausted);
        }
    }
    Ok(output)
}

fn safe_path(path: &str) -> Result<String, Error> {
    let path = path.trim_end_matches('/');
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty()
        || path.starts_with('/')
        || path.len() > 4096
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(Error::UnsafePath);
    }
    Ok(path.into())
}

fn safe_link(path: &str) -> Result<String, Error> {
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(Error::UnsafePath);
    }
    if path.starts_with('/') {
        let _ = safe_path(path.trim_start_matches('/'))?;
    } else {
        let mut depth = 0i32;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => depth -= 1,
                _ => depth += 1,
            }
            if depth < -64 {
                return Err(Error::UnsafePath);
            }
        }
    }
    Ok(path.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink(Vec<String>);

    impl LayerSink for RecordingSink {
        type Error = ();

        fn ensure_directory(&mut self, path: &str, _: Metadata) -> Result<(), Self::Error> {
            self.0.push(format!("directory:{path}"));
            Ok(())
        }

        fn replace_file(
            &mut self,
            path: &str,
            data: &[u8],
            _: Metadata,
        ) -> Result<(), Self::Error> {
            self.0.push(format!("file:{path}:{}", data.len()));
            Ok(())
        }

        fn replace_symlink(
            &mut self,
            path: &str,
            target: &str,
            _: u32,
            _: u32,
        ) -> Result<(), Self::Error> {
            self.0.push(format!("symlink:{path}:{target}"));
            Ok(())
        }

        fn replace_hardlink(&mut self, path: &str, target: &str) -> Result<(), Self::Error> {
            self.0.push(format!("hardlink:{path}:{target}"));
            Ok(())
        }

        fn remove(&mut self, path: &str) -> Result<(), Self::Error> {
            self.0.push(format!("remove:{path}"));
            Ok(())
        }

        fn remove_children(&mut self, path: &str) -> Result<(), Self::Error> {
            self.0.push(format!("opaque:{path}"));
            Ok(())
        }

        fn set_xattrs(
            &mut self,
            path: &str,
            values: &[(String, Vec<u8>)],
        ) -> Result<(), Self::Error> {
            self.0.push(format!("xattrs:{path}:{}", values.len()));
            Ok(())
        }
    }

    fn tar(entries: &[(&str, u8, &[u8], &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, kind, data, link) in entries {
            let mut header = [0u8; BLOCK];
            header[..name.len()].copy_from_slice(name.as_bytes());
            octal(&mut header[100..108], 0o755);
            octal(&mut header[108..116], 0);
            octal(&mut header[116..124], 0);
            octal(&mut header[124..136], data.len() as u64);
            octal(&mut header[136..148], 1);
            header[148..156].fill(b' ');
            header[156] = *kind;
            header[157..157 + link.len()].copy_from_slice(link.as_bytes());
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
            octal(&mut header[148..156], checksum);
            out.extend_from_slice(&header);
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(BLOCK) * BLOCK, 0);
        }
        out.resize(out.len() + BLOCK * 2, 0);
        out
    }

    fn octal(field: &mut [u8], value: u64) {
        field.fill(b'0');
        let text = format!("{value:o}");
        let start = field.len() - text.len() - 1;
        field[start..start + text.len()].copy_from_slice(text.as_bytes());
        field[field.len() - 1] = 0;
    }

    fn pax_record(key: &str, value: &[u8]) -> Vec<u8> {
        let body = 1 + key.len() + 1 + value.len() + 1;
        let length = (1..=20)
            .find_map(|digits| {
                let length = digits + body;
                (length.to_string().len() == digits).then_some(length)
            })
            .unwrap();
        let mut record = format!("{length} {key}=").into_bytes();
        record.extend_from_slice(value);
        record.push(b'\n');
        assert_eq!(record.len(), length);
        record
    }

    fn owned_tar(entries: &[(String, u8, Vec<u8>, String)]) -> Vec<u8> {
        let borrowed: Vec<_> = entries
            .iter()
            .map(|(name, kind, data, link)| (name.as_str(), *kind, data.as_slice(), link.as_str()))
            .collect();
        tar(&borrowed)
    }

    #[test]
    fn layer_whiteouts_links_and_files_are_planned() {
        let layer = tar(&[
            ("etc/", b'5', b"", ""),
            ("etc/config", b'0', b"value", ""),
            ("etc/current", b'2', b"", "config"),
            ("etc/.wh.old", b'0', b"", ""),
            ("tmp/.wh..wh..opq", b'0', b"", ""),
        ]);
        let plan = layer_plan(&layer).unwrap();
        assert!(matches!(plan[0], LayerOperation::Directory { .. }));
        assert!(matches!(plan[1], LayerOperation::File { ref data, .. } if data == b"value"));
        assert!(
            matches!(plan[2], LayerOperation::Symlink { ref target, .. } if target == "config")
        );
        assert_eq!(
            plan[3],
            LayerOperation::Remove {
                path: "etc/old".into()
            }
        );
        let mut sink = RecordingSink::default();
        apply_layer(&plan, &mut sink).unwrap();
        assert_eq!(
            sink.0,
            [
                "directory:etc",
                "file:etc/config:5",
                "symlink:etc/current:config",
                "remove:etc/old",
                "opaque:tmp",
            ]
        );
        assert_eq!(
            plan[4],
            LayerOperation::OpaqueDirectory { path: "tmp".into() }
        );
    }

    #[test]
    fn layer_preserves_binary_and_libarchive_xattrs() {
        let mut attributes = pax_record("SCHILY.xattr.user.binary", &[0, 0xff, 7]);
        attributes.extend_from_slice(&pax_record(
            "LIBARCHIVE.xattr.security.capability",
            b"AQIDBA==",
        ));
        let layer = tar(&[
            ("PaxHeaders/tool", b'x', attributes.as_slice(), ""),
            ("bin/tool", b'0', b"ELF", ""),
        ]);
        let plan = layer_plan(&layer).unwrap();
        assert!(matches!(plan[0], LayerOperation::File { ref path, .. } if path == "bin/tool"));
        assert_eq!(
            plan[1],
            LayerOperation::Xattrs {
                path: "bin/tool".into(),
                values: vec![
                    ("security.capability".into(), vec![1, 2, 3, 4]),
                    ("user.binary".into(), vec![0, 0xff, 7]),
                ],
            }
        );
        let mut sink = RecordingSink::default();
        apply_layer(&plan, &mut sink).unwrap();
        assert_eq!(sink.0, ["file:bin/tool:3", "xattrs:bin/tool:2"]);
        assert_eq!(decode_base64(b"AQ=A"), Err(Error::InvalidTar));
    }

    #[test]
    fn archive_rejects_parent_traversal_and_bad_checksum() {
        assert_eq!(
            layer_plan(&tar(&[("../escape", b'0', b"x", "")])),
            Err(Error::UnsafePath)
        );
        let mut archive = tar(&[("ok", b'0', b"x", "")]);
        archive[10] ^= 1;
        assert!(matches!(tar_entries(&archive), Err(Error::InvalidTar)));
        let duplicate = tar(&[
            ("oci-layout", b'0', br#"{"imageLayoutVersion":"1.0.0"}"#, ""),
            ("oci-layout", b'0', br#"{"imageLayoutVersion":"1.0.0"}"#, ""),
        ]);
        assert_eq!(parse_layout(&duplicate, "amd64"), Err(Error::InvalidLayout));
    }

    #[test]
    fn gzip_decoder_checks_crc_size_and_output_bound() {
        let encoded = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 203, 72, 205, 201, 201, 87, 72, 175, 202, 44, 200,
            24, 101, 141, 178, 70, 89, 195, 148, 5, 0, 158, 234, 210, 126, 232, 3, 0, 0,
        ];
        let expected = b"hello gzip".repeat(100);
        assert_eq!(
            gzip::decompress(&encoded, expected.len()).unwrap(),
            expected
        );
        assert_eq!(
            gzip::decompress(&encoded, expected.len() - 1),
            Err(Error::ResourceExhausted)
        );
        let mut corrupt = encoded;
        corrupt[31] ^= 1;
        assert_eq!(
            gzip::decompress(&corrupt, expected.len()),
            Err(Error::InvalidDigest)
        );
    }

    #[test]
    fn gzip_decoder_accepts_dynamic_huffman_blocks() {
        let encoded = hex_bytes(concat!(
            "1f8b08000000000002ffedcf392b36000000e07792720f189023724c24921c21d764b0484acab5288b01034692ab1421a510250b832219f4094911ca912803914191e9fb1f7a9e7ff00492cadb47b71e228abb162fc2ab06777f4b870e231b963eca269f8ba65e6b97833b8f736783baefeaf6f3d7d316e267e2e6525672766a2e3bbec7d2f71abfa60b6e0632cefab3ae864b3ed75a936fe79b535f36fbaa639eb6475a0aa3df4f37267a9a2ab313427fde1eafcf4f8efe1d9d9c5f3fbefd8426645736f54c6c9cbe4717b68c6c3fc554f76dbea436cfdf26b7ae7d960c5f65f59f650cdc144c7f35eea58f7d775cd6ece4aca4ccc5cdc42fa4ade7efd7dd7507cde61e77062fd7be4e153d4f967d2c35441e0e95feee0e56855f2c7615473c6c8db6972705ee0f56c77bdbea2bf2321363a3c24242c2a2621333f32aeadb7ac7570fee03",
            "eaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaea7fb1fe1f58cf5bd750c30000",
        ));
        let expected = (0..50_000)
            .map(|index| {
                let index = index as u64;
                ((index * index + 31 * index) % 251) as u8
            })
            .collect::<Vec<_>>();
        assert_eq!(
            gzip::decompress(&encoded, expected.len()).unwrap(),
            expected
        );
    }

    fn hex_bytes(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digit = |byte| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    _ => panic!("invalid test fixture"),
                };
                digit(pair[0]) << 4 | digit(pair[1])
            })
            .collect()
    }

    #[test]
    fn complete_layout_selects_platform_and_verifies_every_digest() {
        let layer = tar(&[("bin/", b'5', b"", ""), ("bin/tool", b'0', b"ELF", "")]);
        let layer_digest: [u8; 32] = Sha256::digest(&layer).into();
        let config = format!(
            "{{\"architecture\":\"amd64\",\"os\":\"linux\",\"config\":{{\"Entrypoint\":[\"/bin/tool\"],\"Cmd\":[\"--ok\"],\"Env\":[\"PATH=/bin\"],\"WorkingDir\":\"/\"}},\"rootfs\":{{\"type\":\"layers\",\"diff_ids\":[\"sha256:{}\"]}}}}",
            hex(&layer_digest)
        )
        .into_bytes();
        let config_digest: [u8; 32] = Sha256::digest(&config).into();
        let manifest = format!(
            "{{\"schemaVersion\":2,\"mediaType\":\"application/vnd.oci.image.manifest.v1+json\",\"config\":{{\"mediaType\":\"application/vnd.oci.image.config.v1+json\",\"digest\":\"sha256:{}\",\"size\":{}}},\"layers\":[{{\"mediaType\":\"application/vnd.oci.image.layer.v1.tar\",\"digest\":\"sha256:{}\",\"size\":{}}}]}}",
            hex(&config_digest),
            config.len(),
            hex(&layer_digest),
            layer.len(),
        )
        .into_bytes();
        let manifest_digest: [u8; 32] = Sha256::digest(&manifest).into();
        let index = format!(
            "{{\"schemaVersion\":2,\"manifests\":[{{\"mediaType\":\"application/vnd.oci.image.manifest.v1+json\",\"digest\":\"sha256:{}\",\"size\":{},\"platform\":{{\"os\":\"linux\",\"architecture\":\"amd64\"}}}}]}}",
            hex(&manifest_digest),
            manifest.len(),
        )
        .into_bytes();
        let archive = owned_tar(&[
            (
                "oci-layout".into(),
                b'0',
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
                String::new(),
            ),
            ("index.json".into(), b'0', index, String::new()),
            (
                format!("blobs/sha256/{}", hex(&manifest_digest)),
                b'0',
                manifest,
                String::new(),
            ),
            (
                format!("blobs/sha256/{}", hex(&config_digest)),
                b'0',
                config,
                String::new(),
            ),
            (
                format!("blobs/sha256/{}", hex(&layer_digest)),
                b'0',
                layer,
                String::new(),
            ),
        ]);
        let image = parse_layout(&archive, "amd64").unwrap();
        assert_eq!(image.manifest_digest, manifest_digest);
        assert_eq!(image.process.arguments, ["/bin/tool", "--ok"]);
        assert_eq!(image.process.environment, ["PATH=/bin"]);
        assert_eq!(image.layers.len(), 1);
        assert_eq!(
            parse_layout(&archive, "arm64"),
            Err(Error::UnsupportedPlatform)
        );
    }
}
