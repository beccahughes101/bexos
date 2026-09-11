pub mod codec;
pub const SNAPSHOT_MAGIC: u32 = 0x4258_4854;
pub const SNAPSHOT_VERSION: u16 = 1;
mod handoff;
pub use handoff::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotPhase {
    LiveBulk,
    SwitchDelta,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreservedRegion {
    pub start: u64,
    pub len: u64,
}

impl PreservedRegion {
    pub const fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.len)
    }

    pub fn contains(&self, ptr: u64, len: u64) -> bool {
        let Some(region_end) = self.end() else {
            return false;
        };
        let Some(range_end) = ptr.checked_add(len) else {
            return false;
        };

        ptr >= self.start && range_end <= region_end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotHeader {
    pub magic: u32,
    pub version: u16,
    pub phase: SnapshotPhase,
    pub checksum: u32,
    pub payload_len: u32,
    pub preserved_ptr: u64,
    pub preserved_len: u64,
}

impl SnapshotHeader {
    pub fn new(
        phase: SnapshotPhase,
        payload: &[u8],
        preserved_ptr: u64,
        preserved_len: u64,
    ) -> Result<Self, TransplantError> {
        let payload_len = payload
            .len()
            .try_into()
            .map_err(|_| TransplantError::PayloadTooLarge)?;

        Ok(Self {
            magic: SNAPSHOT_MAGIC,
            version: SNAPSHOT_VERSION,
            phase,
            checksum: checksum(payload),
            payload_len,
            preserved_ptr,
            preserved_len,
        })
    }

    pub fn validate(
        &self,
        expected_phase: SnapshotPhase,
        payload: &[u8],
        preserved_regions: &[PreservedRegion],
    ) -> Result<(), TransplantError> {
        if self.magic != SNAPSHOT_MAGIC {
            return Err(TransplantError::BadMagic);
        }
        if self.version != SNAPSHOT_VERSION {
            return Err(TransplantError::UnsupportedVersion);
        }
        if self.phase != expected_phase {
            return Err(TransplantError::WrongSnapshotPhase);
        }
        if self.payload_len as usize != payload.len() {
            return Err(TransplantError::LengthMismatch);
        }
        if self.checksum != checksum(payload) {
            return Err(TransplantError::ChecksumMismatch);
        }
        if !validate_preserved_range(preserved_regions, self.preserved_ptr, self.preserved_len) {
            return Err(TransplantError::RangeOutsidePreservedRam);
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrozenTaskRecord {
    pub task_id: u64,
    pub entry: u64,
    pub stack_pointer: u64,
    pub state_ptr: u64,
    pub state_len: u64,
}

impl FrozenTaskRecord {
    pub fn validate(&self, preserved_regions: &[PreservedRegion]) -> Result<(), TransplantError> {
        if self.task_id == 0 {
            return Err(TransplantError::InvalidTask);
        }
        if !validate_preserved_range(preserved_regions, self.state_ptr, self.state_len) {
            return Err(TransplantError::RangeOutsidePreservedRam);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityHandleRecord {
    pub object_id: u64,
    pub rights: u32,
    pub owner_task_id: u64,
}

impl CapabilityHandleRecord {
    pub fn validate(&self) -> Result<(), TransplantError> {
        if self.object_id == 0 || self.owner_task_id == 0 {
            return Err(TransplantError::InvalidCapability);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aarch64CpuContextRecord {
    pub cpu_id: u64,
    pub program_counter: u64,
    pub stack_pointer: u64,
    pub pstate: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
    pub vbar_el1: u64,
}

impl Aarch64CpuContextRecord {
    pub fn validate(&self) -> Result<(), TransplantError> {
        if self.program_counter == 0 || self.stack_pointer == 0 || self.vbar_el1 == 0 {
            return Err(TransplantError::InvalidCpuContext);
        }
        if self.stack_pointer & 0xf != 0 {
            return Err(TransplantError::InvalidCpuContext);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransplantError {
    BadMagic,
    UnsupportedVersion,
    ChecksumMismatch,
    LengthMismatch,
    PayloadTooLarge,
    WrongSnapshotPhase,
    RangeOutsidePreservedRam,
    InvalidTask,
    InvalidCapability,
    InvalidCpuContext,
    InvalidRuntimeSnapshot,
    InvalidKernelRange,
    OverlappingKernelRange,
}

pub fn validate_preserved_range(regions: &[PreservedRegion], ptr: u64, len: u64) -> bool {
    len > 0 && regions.iter().any(|region| region.contains(ptr, len))
}

pub fn checksum(bytes: &[u8]) -> u32 {
    let mut value = 0x4258_0005_u32;
    for byte in bytes {
        value = value.rotate_left(5) ^ u32::from(*byte);
    }
    value
}
