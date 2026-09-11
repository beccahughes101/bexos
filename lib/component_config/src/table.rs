use alloc::vec::Vec;

const MAGIC: &[u8; 8] = b"BEXCFG\0\0";
const V1_HEADER_LEN: usize = 16;
const V2_HEADER_LEN: usize = 32;
const ENTRY_FIXED_LEN: usize = 8;
pub const MAX_CONFIG_SNAPSHOT_LEN: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigType {
    Bool = 1,
    Uint32 = 2,
    Uint64 = 3,
    String = 4,
    Bytes = 5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    InvalidHeader,
    InvalidEntry,
    InvalidType,
    MissingField,
    TypeMismatch,
    InvalidUtf8,
    TooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigTable<'a> {
    bytes: &'a [u8],
    header_len: usize,
    count: u32,
    version: u32,
    schema_fingerprint: u64,
    generation: u64,
}

impl<'a> ConfigTable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ConfigError> {
        if bytes.len() > MAX_CONFIG_SNAPSHOT_LEN {
            return Err(ConfigError::TooLarge);
        }
        if bytes.len() < V1_HEADER_LEN || &bytes[..8] != MAGIC {
            return Err(ConfigError::InvalidHeader);
        }
        let version = read_u32(bytes, 8).ok_or(ConfigError::InvalidHeader)?;
        let header_len = match version {
            1 => V1_HEADER_LEN,
            2 => V2_HEADER_LEN,
            _ => return Err(ConfigError::InvalidHeader),
        };
        if bytes.len() < header_len {
            return Err(ConfigError::InvalidHeader);
        }
        let count = read_u32(bytes, 12).ok_or(ConfigError::InvalidHeader)?;
        let schema_fingerprint = if version >= 2 {
            read_u64(bytes, 16).ok_or(ConfigError::InvalidHeader)?
        } else {
            0
        };
        let generation = if version >= 2 {
            read_u64(bytes, 24).ok_or(ConfigError::InvalidHeader)?
        } else {
            0
        };
        let mut names = alloc::collections::BTreeSet::new();
        let mut offset = header_len;
        for _ in 0..count {
            let entry = read_entry(bytes, offset)?;
            let name = core::str::from_utf8(entry.name).map_err(|_| ConfigError::InvalidUtf8)?;
            if name.is_empty() || !names.insert(name) {
                return Err(ConfigError::InvalidEntry);
            }
            match entry.config_type {
                ConfigType::Bool if !matches!(entry.value, [0] | [1]) => {
                    return Err(ConfigError::InvalidEntry);
                }
                ConfigType::Uint32 if entry.value.len() != 4 => {
                    return Err(ConfigError::InvalidEntry);
                }
                ConfigType::Uint64 if entry.value.len() != 8 => {
                    return Err(ConfigError::InvalidEntry);
                }
                ConfigType::String => {
                    core::str::from_utf8(entry.value).map_err(|_| ConfigError::InvalidUtf8)?;
                }
                _ => {}
            }
            offset = entry.next_offset;
        }
        if offset != bytes.len() {
            return Err(ConfigError::InvalidEntry);
        }
        Ok(Self {
            bytes,
            header_len,
            count,
            version,
            schema_fingerprint,
            generation,
        })
    }

    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn schema_fingerprint(&self) -> u64 {
        self.schema_fingerprint
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get_bool(&self, name: &str) -> Result<bool, ConfigError> {
        match self.find(name)? {
            EntryValue {
                config_type: ConfigType::Bool,
                value: [0],
                ..
            } => Ok(false),
            EntryValue {
                config_type: ConfigType::Bool,
                value: [1],
                ..
            } => Ok(true),
            EntryValue {
                config_type: ConfigType::Bool,
                ..
            } => Err(ConfigError::InvalidEntry),
            _ => Err(ConfigError::TypeMismatch),
        }
    }

    pub fn get_u32(&self, name: &str) -> Result<u32, ConfigError> {
        let entry = self.find(name)?;
        if entry.config_type != ConfigType::Uint32 {
            return Err(ConfigError::TypeMismatch);
        }
        read_u32(entry.value, 0).ok_or(ConfigError::InvalidEntry)
    }

    pub fn get_u64(&self, name: &str) -> Result<u64, ConfigError> {
        let entry = self.find(name)?;
        if entry.config_type != ConfigType::Uint64 {
            return Err(ConfigError::TypeMismatch);
        }
        read_u64(entry.value, 0).ok_or(ConfigError::InvalidEntry)
    }

    pub fn get_string(&self, name: &str) -> Result<&'a str, ConfigError> {
        let entry = self.find(name)?;
        if entry.config_type != ConfigType::String {
            return Err(ConfigError::TypeMismatch);
        }
        core::str::from_utf8(entry.value).map_err(|_| ConfigError::InvalidUtf8)
    }

    pub fn get_bytes(&self, name: &str) -> Result<&'a [u8], ConfigError> {
        let entry = self.find(name)?;
        if entry.config_type != ConfigType::Bytes {
            return Err(ConfigError::TypeMismatch);
        }
        Ok(entry.value)
    }

    pub fn entries(&self) -> Vec<(&'a str, ConfigType)> {
        let mut entries = Vec::new();
        let mut offset = self.header_len;
        for _ in 0..self.count {
            let Ok(entry) = read_entry(self.bytes, offset) else {
                break;
            };
            if let Ok(name) = core::str::from_utf8(entry.name) {
                entries.push((name, entry.config_type));
            }
            offset = entry.next_offset;
        }
        entries
    }

    fn find(&self, name: &str) -> Result<EntryValue<'a>, ConfigError> {
        let mut offset = self.header_len;
        for _ in 0..self.count {
            let entry = read_entry(self.bytes, offset)?;
            if entry.name == name.as_bytes() {
                return Ok(entry);
            }
            offset = entry.next_offset;
        }
        Err(ConfigError::MissingField)
    }
}

pub fn encode_config(entries: &[(&str, ConfigType, &[u8])]) -> Vec<u8> {
    encode_config_v1(entries)
}

pub fn encode_config_v1(entries: &[(&str, ConfigType, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, config_type, value) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(*config_type as u8);
        out.push(0);
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(value);
    }
    out
}

pub fn encode_config_v2(
    schema_fingerprint: u64,
    generation: u64,
    entries: &[(&str, ConfigType, &[u8])],
) -> Result<Vec<u8>, ConfigError> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&schema_fingerprint.to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    for (name, config_type, value) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(*config_type as u8);
        out.push(0);
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(value);
    }
    if out.len() > MAX_CONFIG_SNAPSHOT_LEN {
        Err(ConfigError::TooLarge)
    } else {
        ConfigTable::parse(&out)?;
        Ok(out)
    }
}

pub fn schema_fingerprint(fields: &[(&str, ConfigType, bool, u32)]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (name, config_type, required, max_size) in fields {
        for b in name.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= *config_type as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= u64::from(*required);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= *max_size as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct EntryValue<'a> {
    name: &'a [u8],
    config_type: ConfigType,
    value: &'a [u8],
    next_offset: usize,
}

fn read_entry(bytes: &[u8], offset: usize) -> Result<EntryValue<'_>, ConfigError> {
    let fixed = bytes
        .get(offset..offset + ENTRY_FIXED_LEN)
        .ok_or(ConfigError::InvalidEntry)?;
    if fixed[3] != 0 {
        return Err(ConfigError::InvalidEntry);
    }
    let name_len = u16::from_le_bytes([fixed[0], fixed[1]]) as usize;
    let config_type = match fixed[2] {
        1 => ConfigType::Bool,
        2 => ConfigType::Uint32,
        3 => ConfigType::Uint64,
        4 => ConfigType::String,
        5 => ConfigType::Bytes,
        _ => return Err(ConfigError::InvalidType),
    };
    let value_len = u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]) as usize;
    let name_start = offset + ENTRY_FIXED_LEN;
    let value_start = name_start
        .checked_add(name_len)
        .ok_or(ConfigError::InvalidEntry)?;
    let next_offset = value_start
        .checked_add(value_len)
        .ok_or(ConfigError::InvalidEntry)?;
    let name = bytes
        .get(name_start..value_start)
        .ok_or(ConfigError::InvalidEntry)?;
    let value = bytes
        .get(value_start..next_offset)
        .ok_or(ConfigError::InvalidEntry)?;
    Ok(EntryValue {
        name,
        config_type,
        value,
        next_offset,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let raw = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let raw = bytes.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

impl ConfigType {
    pub fn scalar_bytes(self, value: ConfigScalar<'_>) -> Result<Vec<u8>, ConfigError> {
        let mut out = Vec::new();
        match (self, value) {
            (Self::Bool, ConfigScalar::Bool(value)) => out.push(u8::from(value)),
            (Self::Uint32, ConfigScalar::Uint32(value)) => {
                out.extend_from_slice(&value.to_le_bytes())
            }
            (Self::Uint64, ConfigScalar::Uint64(value)) => {
                out.extend_from_slice(&value.to_le_bytes())
            }
            (Self::String, ConfigScalar::String(value)) => out.extend_from_slice(value.as_bytes()),
            (Self::Bytes, ConfigScalar::Bytes(value)) => out.extend_from_slice(value),
            _ => return Err(ConfigError::TypeMismatch),
        }
        Ok(out)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScalar<'a> {
    Bool(bool),
    Uint32(u32),
    Uint64(u64),
    String(&'a str),
    Bytes(&'a [u8]),
}
