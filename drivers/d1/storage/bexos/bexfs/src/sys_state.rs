use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bexos_package_version::{HealthCheckStatus, SemVer};

pub const SYS_STATE_MAGIC: [u8; 8] = *b"BEXSYS01";
pub const SYS_STATE_VERSION: u32 = 1;
pub const SYS_STATE_V1_BYTES: usize = 40;
pub const SYS_STATE_V2_MAGIC: [u8; 8] = *b"BEXSYS02";
pub const SYS_STATE_V2_VERSION: u32 = 2;
pub const SYS_STATE_V2_RECORD_BYTES: usize = 64;
pub const SYS_STATE_V2_BYTES: usize = SYS_STATE_V2_RECORD_BYTES * 2;
pub const PIN_RECORD_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Slot {
    A = 0,
    B = 1,
}

pub fn pinned_apps_path(slot: Slot) -> &'static str {
    match slot {
        Slot::A => "slot_a/pinned_apps.redb",
        Slot::B => "slot_b/pinned_apps.redb",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivePackagePin {
    pub package_id: String,
    pub pinned_version: SemVer,
    pub content_blake3: [u8; 32],
    pub is_critical_boot_app: bool,
    pub health_check_status: HealthCheckStatus,
    pub rollback_target_version: Option<SemVer>,
}

impl ActivePackagePin {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(PIN_RECORD_VERSION);
        put_string(&mut out, &self.package_id);
        put_semver(&mut out, &self.pinned_version);
        out.extend_from_slice(&self.content_blake3);
        out.push(self.is_critical_boot_app as u8);
        out.push(self.health_check_status.to_u8());
        match &self.rollback_target_version {
            Some(version) => {
                out.push(1);
                put_semver(&mut out, version);
            }
            None => out.push(0),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SysStateError> {
        let mut cursor = PinCursor { bytes, offset: 0 };
        if cursor.byte()? != PIN_RECORD_VERSION {
            return Err(SysStateError::UnsupportedVersion);
        }
        let package_id = cursor.string(128)?;
        let pinned_version = cursor.semver()?;
        let content_blake3 = cursor.exact_32()?;
        let is_critical_boot_app = cursor.byte()? != 0;
        let health_check_status =
            HealthCheckStatus::from_u8(cursor.byte()?).ok_or(SysStateError::InvalidPin)?;
        let rollback_target_version = if cursor.byte()? != 0 {
            Some(cursor.semver()?)
        } else {
            None
        };
        if cursor.offset != bytes.len() {
            return Err(SysStateError::InvalidPin);
        }
        Ok(Self {
            package_id,
            pinned_version,
            content_blake3,
            is_critical_boot_app,
            health_check_status,
            rollback_target_version,
        })
    }
}

impl Slot {
    fn decode(value: u8) -> Result<Self, SysStateError> {
        match value {
            0 => Ok(Self::A),
            1 => Ok(Self::B),
            _ => Err(SysStateError::InvalidSlot),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysStateV1 {
    pub generation: u64,
    pub active_slot: Slot,
    pub tries_remaining: [u8; 2],
    pub last_known_good_slot: Slot,
    pub last_known_good_version: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysStateV2 {
    pub generation: u64,
    pub active_slot: Slot,
    pub tries_remaining: [u8; 2],
    pub last_known_good_slot: Slot,
    pub last_known_good_version: u64,
    pub sequence: u64,
    pub pending_generation: u64,
    pub pending_flags: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SysStateError {
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion,
    InvalidSlot,
    BadChecksum,
    InvalidPin,
}

impl SysStateV1 {
    pub const fn initial() -> Self {
        Self {
            generation: 1,
            active_slot: Slot::A,
            tries_remaining: [3, 3],
            last_known_good_slot: Slot::A,
            last_known_good_version: 1,
        }
    }

    pub fn encode(self) -> [u8; SYS_STATE_V1_BYTES] {
        let mut out = [0u8; SYS_STATE_V1_BYTES];
        out[0..8].copy_from_slice(&SYS_STATE_MAGIC);
        out[8..12].copy_from_slice(&SYS_STATE_VERSION.to_le_bytes());
        out[12..20].copy_from_slice(&self.generation.to_le_bytes());
        out[20] = self.active_slot as u8;
        out[21..23].copy_from_slice(&self.tries_remaining);
        out[23] = self.last_known_good_slot as u8;
        out[24..32].copy_from_slice(&self.last_known_good_version.to_le_bytes());
        let checksum = rosefs_core::journal::crc32c::crc32c(&out[..36]);
        out[36..40].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SysStateError> {
        if bytes.len() != SYS_STATE_V1_BYTES {
            return Err(SysStateError::InvalidLength);
        }
        if bytes[0..8] != SYS_STATE_MAGIC {
            return Err(SysStateError::InvalidMagic);
        }
        if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != SYS_STATE_VERSION {
            return Err(SysStateError::UnsupportedVersion);
        }
        let expected = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        if rosefs_core::journal::crc32c::crc32c(&bytes[..36]) != expected {
            return Err(SysStateError::BadChecksum);
        }
        Ok(Self {
            generation: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            active_slot: Slot::decode(bytes[20])?,
            tries_remaining: [bytes[21], bytes[22]],
            last_known_good_slot: Slot::decode(bytes[23])?,
            last_known_good_version: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
        })
    }
}

impl From<SysStateV1> for SysStateV2 {
    fn from(value: SysStateV1) -> Self {
        Self {
            generation: value.generation,
            active_slot: value.active_slot,
            tries_remaining: value.tries_remaining,
            last_known_good_slot: value.last_known_good_slot,
            last_known_good_version: value.last_known_good_version,
            sequence: value.generation,
            pending_generation: 0,
            pending_flags: 0,
        }
    }
}

impl From<SysStateV2> for SysStateV1 {
    fn from(value: SysStateV2) -> Self {
        Self {
            generation: value.generation,
            active_slot: value.active_slot,
            tries_remaining: value.tries_remaining,
            last_known_good_slot: value.last_known_good_slot,
            last_known_good_version: value.last_known_good_version,
        }
    }
}

impl SysStateV2 {
    pub const fn initial() -> Self {
        Self {
            generation: 1,
            active_slot: Slot::A,
            tries_remaining: [3, 3],
            last_known_good_slot: Slot::A,
            last_known_good_version: 1,
            sequence: 1,
            pending_generation: 0,
            pending_flags: 0,
        }
    }

    pub fn from_v1(value: SysStateV1) -> Self {
        value.into()
    }

    pub fn as_v1(self) -> SysStateV1 {
        self.into()
    }

    pub fn encode(self) -> [u8; SYS_STATE_V2_BYTES] {
        let mut out = [0u8; SYS_STATE_V2_BYTES];
        encode_v2_record(self, self.sequence, &mut out[..SYS_STATE_V2_RECORD_BYTES]);
        encode_v2_record(
            self,
            self.sequence.saturating_sub(1),
            &mut out[SYS_STATE_V2_RECORD_BYTES..],
        );
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SysStateError> {
        if bytes.len() != SYS_STATE_V2_BYTES {
            return Err(SysStateError::InvalidLength);
        }
        let a = decode_v2_record(&bytes[..SYS_STATE_V2_RECORD_BYTES]);
        let b = decode_v2_record(&bytes[SYS_STATE_V2_RECORD_BYTES..]);
        match (a, b) {
            (Ok(a), Ok(b)) => Ok(if a.sequence >= b.sequence { a } else { b }),
            (Ok(a), Err(_)) => Ok(a),
            (Err(_), Ok(b)) => Ok(b),
            (Err(e), Err(_)) => Err(e),
        }
    }

    pub fn decode_any(bytes: &[u8]) -> Result<Self, SysStateError> {
        match bytes.len() {
            SYS_STATE_V1_BYTES => SysStateV1::decode(bytes).map(Self::from_v1),
            SYS_STATE_V2_BYTES => Self::decode(bytes),
            _ => Err(SysStateError::InvalidLength),
        }
    }
}

fn encode_v2_record(state: SysStateV2, sequence: u64, out: &mut [u8]) {
    out[0..8].copy_from_slice(&SYS_STATE_V2_MAGIC);
    out[8..12].copy_from_slice(&SYS_STATE_V2_VERSION.to_le_bytes());
    out[12..20].copy_from_slice(&sequence.to_le_bytes());
    out[20..28].copy_from_slice(&state.generation.to_le_bytes());
    out[28] = state.active_slot as u8;
    out[29..31].copy_from_slice(&state.tries_remaining);
    out[31] = state.last_known_good_slot as u8;
    out[32..40].copy_from_slice(&state.last_known_good_version.to_le_bytes());
    out[40..48].copy_from_slice(&state.pending_generation.to_le_bytes());
    out[48..56].copy_from_slice(&state.pending_flags.to_le_bytes());
    let checksum = rosefs_core::journal::crc32c::crc32c(&out[..60]);
    out[60..64].copy_from_slice(&checksum.to_le_bytes());
}

fn decode_v2_record(bytes: &[u8]) -> Result<SysStateV2, SysStateError> {
    if bytes.len() != SYS_STATE_V2_RECORD_BYTES {
        return Err(SysStateError::InvalidLength);
    }
    if bytes[0..8] != SYS_STATE_V2_MAGIC {
        return Err(SysStateError::InvalidMagic);
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != SYS_STATE_V2_VERSION {
        return Err(SysStateError::UnsupportedVersion);
    }
    let expected = u32::from_le_bytes(bytes[60..64].try_into().unwrap());
    if rosefs_core::journal::crc32c::crc32c(&bytes[..60]) != expected {
        return Err(SysStateError::BadChecksum);
    }
    Ok(SysStateV2 {
        sequence: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        generation: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        active_slot: Slot::decode(bytes[28])?,
        tries_remaining: [bytes[29], bytes[30]],
        last_known_good_slot: Slot::decode(bytes[31])?,
        last_known_good_version: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
        pending_generation: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
        pending_flags: u64::from_le_bytes(bytes[48..56].try_into().unwrap()),
    })
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    put_u16(out, value.len() as u16);
    out.extend_from_slice(value.as_bytes());
}

fn put_semver(out: &mut Vec<u8>, version: &SemVer) {
    for value in [version.major, version.minor, version.patch, version.build] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    put_string(out, &version.prerelease);
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct PinCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl PinCursor<'_> {
    fn byte(&mut self) -> Result<u8, SysStateError> {
        let byte = *self
            .bytes
            .get(self.offset)
            .ok_or(SysStateError::InvalidPin)?;
        self.offset += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, SysStateError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, SysStateError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn string(&mut self, max: usize) -> Result<String, SysStateError> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(SysStateError::InvalidPin);
        }
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes)
            .map(ToString::to_string)
            .map_err(|_| SysStateError::InvalidPin)
    }

    fn semver(&mut self) -> Result<SemVer, SysStateError> {
        Ok(SemVer {
            major: self.u32()?,
            minor: self.u32()?,
            patch: self.u32()?,
            build: self.u32()?,
            prerelease: self.string(64)?,
        })
    }

    fn exact_32(&mut self) -> Result<[u8; 32], SysStateError> {
        self.take(32)?
            .try_into()
            .map_err(|_| SysStateError::InvalidPin)
    }

    fn take(&mut self, len: usize) -> Result<&[u8], SysStateError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(SysStateError::InvalidPin)?;
        if end > self.bytes.len() {
            return Err(SysStateError::InvalidPin);
        }
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
}
