use super::{Aarch64CpuContextRecord, TransplantError, checksum, codec::Writer};

pub const HANDOFF_MAGIC: u64 = u64::from_le_bytes(*b"BEXKH003");
pub const HANDOFF_VERSION: u64 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Aarch64SystemRegisters {
    pub mair: u64,
    pub tcr: u64,
    pub sctlr: u64,
    pub cpacr: u64,
    pub cntkctl: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelRange {
    pub start: u64,
    pub len: u64,
}
impl KernelRange {
    pub const fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }
    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.len)
    }
    pub fn is_page_aligned(&self) -> bool {
        self.start % 4096 == 0 && self.len % 4096 == 0
    }
    pub fn contains(&self, other: Self) -> bool {
        self.len > 0
            && other.len > 0
            && self.start <= other.start
            && self.end().zip(other.end()).is_some_and(|(a, b)| b <= a)
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self.end()
            .zip(other.end())
            .is_none_or(|(a, b)| self.start < b && other.start < a)
    }
}

/// Fixed-width header. The payload is the canonical codec, never a native Runtime.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct KernelTransplantHandoff {
    pub magic: u64,
    pub version: u64,
    pub generation: u64,
    pub old_kernel: KernelRange,
    pub old_reclaim: KernelRange,
    pub replacement: KernelRange,
    pub handoff: KernelRange,
    pub snapshot: KernelRange,
    pub replacement_entry: u64,
    pub cpu: Aarch64CpuContextRecord,
    pub system: Aarch64SystemRegisters,
    pub switch_authorized: u64,
    pub artifact_hash: [u8; 32],
    pub snapshot_checksum: u64,
    pub checksum: u64,
    /// Appended to preserve the supported version 3 ARM layout.
    pub architecture: u64,
    pub architecture_state: [u64; 16],
}
impl KernelTransplantHandoff {
    pub fn seal(&mut self) {
        self.checksum = self.computed_checksum() as u64;
    }
    pub fn validate(&self) -> Result<(), TransplantError> {
        if self.magic != HANDOFF_MAGIC {
            return Err(TransplantError::BadMagic);
        }
        if self.version != HANDOFF_VERSION
            && !(self.version == 3 && crate::runtime::Context::ARCHITECTURE == 1)
        {
            return Err(TransplantError::UnsupportedVersion);
        }
        if self.version >= 4 && self.architecture != crate::runtime::Context::ARCHITECTURE {
            return Err(TransplantError::InvalidCpuContext);
        }
        if self.checksum != self.computed_checksum() as u64 {
            return Err(TransplantError::ChecksumMismatch);
        }
        let ranges = [
            self.old_kernel,
            self.old_reclaim,
            self.replacement,
            self.handoff,
        ];
        for r in ranges {
            if r.len == 0 || !r.is_page_aligned() || r.end().is_none() {
                return Err(TransplantError::InvalidKernelRange);
            }
        }
        if !self.old_kernel.contains(self.old_reclaim)
            || !KernelRange::new(
                bexos_boot::UPDATE_BASE,
                bexos_boot::UPDATE_END - bexos_boot::UPDATE_BASE,
            )
            .contains(self.replacement)
            || !KernelRange::new(
                bexos_boot::UPDATE_SNAPSHOT,
                bexos_boot::UPDATE_SNAPSHOT_END - bexos_boot::UPDATE_SNAPSHOT,
            )
            .contains(self.snapshot)
            || !self
                .replacement
                .contains(KernelRange::new(self.replacement_entry, 4))
        {
            return Err(TransplantError::InvalidKernelRange);
        }
        for (i, a) in [
            self.old_kernel,
            self.replacement,
            self.handoff,
            self.snapshot,
        ]
        .iter()
        .enumerate()
        {
            for b in [
                self.old_kernel,
                self.replacement,
                self.handoff,
                self.snapshot,
            ]
            .iter()
            .skip(i + 1)
            {
                if a.overlaps(b) {
                    return Err(TransplantError::OverlappingKernelRange);
                }
            }
        }
        if self.switch_authorized > 1 {
            return Err(TransplantError::InvalidRuntimeSnapshot);
        }
        self.cpu.validate()
    }
    pub fn validate_snapshot(&self, bytes: &[u8]) -> Result<(), TransplantError> {
        self.validate()?;
        if bytes.len() as u64 != self.snapshot.len {
            return Err(TransplantError::LengthMismatch);
        }
        if checksum(bytes) as u64 != self.snapshot_checksum {
            return Err(TransplantError::ChecksumMismatch);
        }
        Ok(())
    }
    fn computed_checksum(&self) -> u32 {
        // Do not hash repr(C) padding: its contents are unspecified.
        let mut bytes = [0u8; 512];
        let mut w = Writer::new(&mut bytes);
        for n in [self.magic, self.version, self.generation] {
            w.word(n).unwrap();
        }
        for r in [
            self.old_kernel,
            self.old_reclaim,
            self.replacement,
            self.handoff,
            self.snapshot,
        ] {
            w.word(r.start).unwrap();
            w.word(r.len).unwrap();
        }
        for n in [
            self.replacement_entry,
            self.cpu.cpu_id,
            self.cpu.program_counter,
            self.cpu.stack_pointer,
            self.cpu.pstate,
            self.cpu.ttbr0_el1,
            self.cpu.ttbr1_el1,
            self.cpu.vbar_el1,
            self.snapshot_checksum,
        ] {
            w.word(n).unwrap();
        }
        w.bytes(&self.artifact_hash).unwrap();
        for n in [
            self.system.mair,
            self.system.tcr,
            self.system.sctlr,
            self.system.cpacr,
            self.system.cntkctl,
            self.switch_authorized,
        ] {
            w.word(n).unwrap();
        }
        if self.version >= 4 {
            w.word(self.architecture).unwrap();
            for n in self.architecture_state {
                w.word(n).unwrap();
            }
        }
        let len = w.len();
        checksum(&bytes[..len])
    }
}
