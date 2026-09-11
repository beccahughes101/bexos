#![cfg_attr(not(test), no_std)]

pub mod framebuffer;
mod handoff_compat;
pub use framebuffer::BootFramebuffer;

// ============================================================================
// EL0 USERSPACE CANONICAL VIRTUAL MEMORY LAYOUT (Fixed across all platforms)
// ============================================================================
pub const USER_IMAGE_BASE: u64 = if cfg!(bexos_arch_x86_64) {
    0x40000000
} else {
    0x8000_0000
};
pub const USER_START: u64 = USER_IMAGE_BASE; // Lower 2 GiB is reserved for EL1 identity mappings.
pub const USER_END: u64 = 0x80_0000_0000; // 512GB Canonical Range
pub const USER_HEAP_VMAR_BASE: u64 = 0x0003_0000_0000;
pub const USER_HEAP_VMAR_SIZE: u64 = 64 * 1024 * 1024 * 1024; // 64 GB
pub const USER_HEAP_CHUNK_SIZE: u64 = 2 * 1024 * 1024; // 2 MB Arenas

pub const USER_STACK_TOP: u64 = 0x007f_f000_0000;
pub const USER_STACK_SIZE: u64 = 256 * 1024; // 256 KB Stack
pub const USER_TLS_TCB_SIZE: u64 = 16; // AArch64 ELF TLS variant-I bias.

const _: () =
    assert!(USER_HEAP_VMAR_BASE + USER_HEAP_VMAR_SIZE <= USER_STACK_TOP - USER_STACK_SIZE);
const _: () = assert!(USER_STACK_TOP <= USER_END);

pub const APPD_PATH: &str = "/boot/pkg/bexos.platform.appd/bin/appd";

// ============================================================================
// BOOT HANDOFF PROTOCOL
// ============================================================================
pub const ARCHITECTURE_ID: u64 = if cfg!(bexos_arch_x86_64) { 2 } else { 1 };
pub const PAGE: u64 = 4096;
pub const HANDOFF_MAGIC: u64 = u64::from_le_bytes(*b"BEXRAM01");
pub const BOOT_HANDOFF_VERSION: u64 = 5;
pub const BOOT_HANDOFF_EVIDENCE_VERSION: u64 = 3;
pub const BOOT_HANDOFF_LEGACY_VERSION: u64 = 2;
pub const MAX_BOOT_CPUS: u64 = 64;
pub const RAM_START: u64 = if cfg!(bexos_arch_x86_64) {
    0x1000000
} else {
    0x4000_0000
};
pub const RAM_END: u64 = if cfg!(bexos_arch_x86_64) {
    0x30000000
} else {
    0x8000_0000
};
pub const HANDOFF_ADDR: u64 = if cfg!(bexos_arch_x86_64) {
    0x1000000
} else {
    0x4010_0000
};
pub const KERNEL_START: u64 = if cfg!(bexos_arch_x86_64) {
    0x2000000
} else {
    0x4020_0000
};
pub const BOOTFS_ADDR: u64 = if cfg!(bexos_arch_x86_64) {
    0x8000000
} else {
    0x4400_0000
};
pub const BOOT_EVIDENCE_ADDR: u64 = if cfg!(bexos_arch_x86_64) {
    0x7f00000
} else {
    0x43f0_0000
};
pub const VBMETA_ADDR: u64 = if cfg!(bexos_arch_x86_64) {
    0x7e00000
} else {
    0x43e0_0000
};
pub const MAX_VBMETA_LEN: u64 = 64 * 1024;
pub const MAX_BOOT_EVIDENCE_LEN: u64 = PAGE;
pub const UPDATE_BASE: u64 = if cfg!(bexos_arch_x86_64) {
    0x10000000
} else {
    0x4200_0000
};
pub const UPDATE_END: u64 = if cfg!(bexos_arch_x86_64) {
    0x11000000
} else {
    0x4300_0000
};
pub const UPDATE_SNAPSHOT: u64 = if cfg!(bexos_arch_x86_64) {
    0x11000000
} else {
    0x4300_0000
};
pub const UPDATE_SNAPSHOT_END: u64 = if cfg!(bexos_arch_x86_64) {
    0x11400000
} else {
    0x4340_0000
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct BootHandoff {
    pub magic: u64,
    pub version: u64,
    pub ram_start: u64,
    pub ram_end: u64,
    pub bootfs_addr: u64,
    pub bootfs_len: u64,
    pub update_base: u64,
    pub update_len: u64,
    pub max_cpus: u64,
    pub entropy_seed_valid: u64,
    pub entropy_seed: [u64; 4],
    pub boot_evidence_addr: u64,
    pub boot_evidence_len: u64,
    pub secure_monitor_call_header: u64,
    pub secure_monitor_features: u64,
    pub framebuffer: BootFramebuffer,
}

impl BootHandoff {
    pub const WORDS: usize = 24;

    pub const fn qemu_default(bootfs_len: u64, max_cpus: u64) -> Self {
        Self {
            magic: HANDOFF_MAGIC,
            version: BOOT_HANDOFF_EVIDENCE_VERSION,
            ram_start: RAM_START,
            ram_end: RAM_END,
            bootfs_addr: BOOTFS_ADDR,
            bootfs_len,
            update_base: UPDATE_BASE,
            update_len: UPDATE_SNAPSHOT_END - UPDATE_BASE,
            max_cpus,
            entropy_seed_valid: 0,
            entropy_seed: [0; 4],
            boot_evidence_addr: BOOT_EVIDENCE_ADDR,
            boot_evidence_len: BootEvidenceV1::BYTES as u64,
            secure_monitor_call_header: 0,
            secure_monitor_features: 0,
            framebuffer: BootFramebuffer::NONE,
        }
    }

    /// Q35 guest RAM excludes the Multiboot loader and firmware workspace.
    pub const fn qemu_x86_64(bootfs_len: u64, max_cpus: u64) -> Self {
        let mut handoff = Self::qemu_default(bootfs_len, max_cpus);
        handoff.ram_start = 0x0100_0000;
        handoff.ram_end = 0x3000_0000;
        // Large development images include both native and WASM runtimes. Keep
        // them clear of the fixed replacement image and snapshot reservation.
        handoff.bootfs_addr = if bootfs_len > 0x0800_0000 {
            0x1400_0000
        } else {
            0x0800_0000
        };
        handoff.boot_evidence_addr = 0x07f0_0000;
        handoff.update_base = 0x1000_0000;
        handoff.update_len = 0x0140_0000;
        handoff
    }

    pub const fn words(&self) -> [u64; Self::WORDS] {
        [
            self.magic,
            self.version,
            self.ram_start,
            self.ram_end,
            self.bootfs_addr,
            self.bootfs_len,
            self.update_base,
            self.update_len,
            self.max_cpus,
            self.entropy_seed_valid,
            self.entropy_seed[0],
            self.entropy_seed[1],
            self.entropy_seed[2],
            self.entropy_seed[3],
            self.boot_evidence_addr,
            self.boot_evidence_len,
            self.secure_monitor_call_header,
            self.secure_monitor_features,
            self.framebuffer.address,
            self.framebuffer.length,
            self.framebuffer.width,
            self.framebuffer.height,
            self.framebuffer.stride,
            self.framebuffer.format,
        ]
    }

    pub fn validate(&self, kernel_start: u64, kernel_end: u64) -> bool {
        // 1. Header & Version check
        if self.magic != HANDOFF_MAGIC
            || !matches!(
                self.version,
                1 | BOOT_HANDOFF_LEGACY_VERSION | 3 | 4 | BOOT_HANDOFF_VERSION
            )
        {
            return false;
        }

        if self.version >= 3 {
            if self.entropy_seed_valid > 1 {
                return false;
            }
            if self.entropy_seed_valid == 1 && self.entropy_seed.iter().all(|word| *word == 0) {
                return false;
            }
        }

        // 2. CPU count sanity
        let cpus = self.effective_max_cpus();
        if cpus == 0 || cpus > MAX_BOOT_CPUS {
            return false;
        }

        // 3. RAM boundaries alignment
        if self.ram_start >= self.ram_end
            || !self.ram_start.is_multiple_of(PAGE)
            || !self.ram_end.is_multiple_of(PAGE)
        {
            return false;
        }

        // 4. Kernel placement within RAM
        if kernel_start < self.ram_start || kernel_end > self.ram_end || kernel_start >= kernel_end
        {
            return false;
        }

        // 5. BootFS validation within RAM (no overlap with kernel)
        if self.bootfs_len == 0
            || !self.bootfs_len.is_multiple_of(PAGE)
            || !self.bootfs_addr.is_multiple_of(PAGE)
        {
            return false;
        }
        let bootfs_end = match self.bootfs_addr.checked_add(self.bootfs_len) {
            Some(end) if end <= self.ram_end => end,
            _ => return false,
        };
        if self.bootfs_addr < kernel_end && bootfs_end > kernel_start {
            return false; // Kernel overlaps BootFS
        }

        // 6. Update workspace reservation validation (if present)
        if self.update_len > 0 {
            if !self.update_base.is_multiple_of(PAGE) || !self.update_len.is_multiple_of(PAGE) {
                return false;
            }
            let update_end = match self.update_base.checked_add(self.update_len) {
                Some(end) if end <= self.ram_end => end,
                _ => return false,
            };
            // Ensure update region doesn't overlap kernel or bootfs
            if (self.update_base < kernel_end && update_end > kernel_start)
                || (self.update_base < bootfs_end && update_end > self.bootfs_addr)
            {
                return false;
            }
        }

        if self.version >= 3 {
            if self.boot_evidence_len == 0
                || self.boot_evidence_len > MAX_BOOT_EVIDENCE_LEN
                || !self.boot_evidence_addr.is_multiple_of(PAGE)
                || !self.boot_evidence_len.is_multiple_of(8)
            {
                return false;
            }
            let evidence_end = match self.boot_evidence_addr.checked_add(self.boot_evidence_len) {
                Some(end) if end <= self.ram_end => end,
                _ => return false,
            };
            if (self.boot_evidence_addr < kernel_end && evidence_end > kernel_start)
                || (self.boot_evidence_addr < bootfs_end && evidence_end > self.bootfs_addr)
                || (self.update_len > 0
                    && self.boot_evidence_addr < self.update_base + self.update_len
                    && evidence_end > self.update_base)
            {
                return false;
            }
        }

        if self.version >= 4 {
            if self.secure_monitor_call_header != 0 {
                if self.secure_monitor_features == 0 {
                    return false;
                }
            } else if self.secure_monitor_features != 0 {
                return false;
            }
        }

        true
    }

    pub const fn effective_max_cpus(&self) -> u64 {
        if self.version == 1 { 1 } else { self.max_cpus }
    }
}

pub const BOOT_EVIDENCE_MAGIC: u64 = u64::from_le_bytes(*b"BEXEV001");
pub const BOOT_EVIDENCE_VERSION: u32 = 1;
pub const BOOT_EVIDENCE_VERIFIED_BL33_VERSION: u32 = 2;
pub const BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION: u32 = 3;
pub const BOOT_EVIDENCE_FLAG_SECURE_BOOT: u64 = 1 << 0;
pub const BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK: u64 = 1 << 1;
pub const BOOT_EVIDENCE_FLAG_IOMMU_STRICT: u64 = 1 << 2;
pub const BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR: u64 = 1 << 3;
/// The resident recovery owner authenticated the committed firmware selection
/// before admitting this guest. Covered by the monitor's private evidence seal.
pub const BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION: u64 = 1 << 4;
pub const DEV_BOOT_PUBLIC_KEY: [u8; 32] = [
    0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab, 0x6c,
    0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81, 0xdb, 0x12,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct BootEvidenceV1 {
    pub magic: u64,
    pub version: u32,
    pub header_bytes: u32,
    pub generation: u64,
    pub flags: u64,
    pub kernel_sha256: [u8; 32],
    pub bootfs_sha256: [u8; 32],
    pub policy_sha256: [u8; 32],
    pub orchestrator_sha256: [u8; 32],
    pub event_log_addr: u64,
    pub event_log_len: u64,
    pub public_key: [u8; 32],
    pub signature: [u8; 64],
}

impl BootEvidenceV1 {
    pub const BYTES: usize = 272;
    pub const SIGNED_BYTES: usize = 160;

    pub const fn qemu_dev(
        generation: u64,
        flags: u64,
        kernel_sha256: [u8; 32],
        bootfs_sha256: [u8; 32],
        policy_sha256: [u8; 32],
        orchestrator_sha256: [u8; 32],
    ) -> Self {
        Self {
            magic: BOOT_EVIDENCE_MAGIC,
            version: BOOT_EVIDENCE_VERSION,
            header_bytes: Self::BYTES as u32,
            generation,
            flags,
            kernel_sha256,
            bootfs_sha256,
            policy_sha256,
            orchestrator_sha256,
            event_log_addr: 0,
            event_log_len: 0,
            public_key: DEV_BOOT_PUBLIC_KEY,
            signature: [0; 64],
        }
    }

    pub const fn verified_bl33(
        generation: u64,
        flags: u64,
        kernel_sha256: [u8; 32],
        bootfs_sha256: [u8; 32],
        policy_sha256: [u8; 32],
        orchestrator_sha256: [u8; 32],
        verified_boot_key: [u8; 32],
    ) -> Self {
        Self {
            magic: BOOT_EVIDENCE_MAGIC,
            version: BOOT_EVIDENCE_VERIFIED_BL33_VERSION,
            header_bytes: Self::BYTES as u32,
            generation,
            flags,
            kernel_sha256,
            bootfs_sha256,
            policy_sha256,
            orchestrator_sha256,
            event_log_addr: 0,
            event_log_len: 0,
            // Version 2 is protected by the authenticated BL33-to-kernel
            // control flow rather than this legacy Ed25519 signature field.
            // Carry the digest of the AVB root here for KeyMint boot info.
            public_key: verified_boot_key,
            signature: [0; 64],
        }
    }

    /// Version 3 must be confirmed against the resident x86 monitor's private
    /// evidence seal. A copy of this record alone is never boot authority.
    pub const fn verified_monitor(
        generation: u64,
        kernel_sha256: [u8; 32],
        bootfs_sha256: [u8; 32],
        policy_sha256: [u8; 32],
        secure_runtime_sha256: [u8; 32],
        root_sha256: [u8; 32],
    ) -> Self {
        let mut evidence = Self::verified_bl33(
            generation,
            BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
                | BOOT_EVIDENCE_FLAG_IOMMU_STRICT
                | BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR,
            kernel_sha256,
            bootfs_sha256,
            policy_sha256,
            secure_runtime_sha256,
            root_sha256,
        );
        evidence.version = BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION;
        evidence
    }

    pub fn encode_unsigned(&self, out: &mut [u8; Self::SIGNED_BYTES]) {
        out[0..8].copy_from_slice(&self.magic.to_le_bytes());
        out[8..12].copy_from_slice(&self.version.to_le_bytes());
        out[12..16].copy_from_slice(&self.header_bytes.to_le_bytes());
        out[16..24].copy_from_slice(&self.generation.to_le_bytes());
        out[24..32].copy_from_slice(&self.flags.to_le_bytes());
        out[32..64].copy_from_slice(&self.kernel_sha256);
        out[64..96].copy_from_slice(&self.bootfs_sha256);
        out[96..128].copy_from_slice(&self.policy_sha256);
        out[128..160].copy_from_slice(&self.orchestrator_sha256);
    }

    pub fn encode(&self, out: &mut [u8; Self::BYTES]) {
        let mut signed = [0; Self::SIGNED_BYTES];
        self.encode_unsigned(&mut signed);
        out[..Self::SIGNED_BYTES].copy_from_slice(&signed);
        out[160..168].copy_from_slice(&self.event_log_addr.to_le_bytes());
        out[168..176].copy_from_slice(&self.event_log_len.to_le_bytes());
        out[176..208].copy_from_slice(&self.public_key);
        out[208..272].copy_from_slice(&self.signature);
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::BYTES {
            return None;
        }
        let mut evidence = Self {
            magic: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            version: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
            header_bytes: u32::from_le_bytes(bytes[12..16].try_into().ok()?),
            generation: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            flags: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
            kernel_sha256: bytes[32..64].try_into().ok()?,
            bootfs_sha256: bytes[64..96].try_into().ok()?,
            policy_sha256: bytes[96..128].try_into().ok()?,
            orchestrator_sha256: bytes[128..160].try_into().ok()?,
            event_log_addr: u64::from_le_bytes(bytes[160..168].try_into().ok()?),
            event_log_len: u64::from_le_bytes(bytes[168..176].try_into().ok()?),
            public_key: bytes[176..208].try_into().ok()?,
            signature: [0; 64],
        };
        evidence.signature.copy_from_slice(&bytes[208..272]);
        evidence.basic_validate().then_some(evidence)
    }

    pub fn basic_validate(&self) -> bool {
        self.magic == BOOT_EVIDENCE_MAGIC
            && matches!(
                self.version,
                BOOT_EVIDENCE_VERSION
                    | BOOT_EVIDENCE_VERIFIED_BL33_VERSION
                    | BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION
            )
            && self.header_bytes as usize == Self::BYTES
            && self.generation > 0
            && ((self.version == BOOT_EVIDENCE_VERSION && self.public_key == DEV_BOOT_PUBLIC_KEY)
                || (matches!(
                    self.version,
                    BOOT_EVIDENCE_VERIFIED_BL33_VERSION | BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION
                ) && self.public_key != [0; 32]
                    && self.signature == [0; 64]))
    }
}

#[inline(always)]
pub const fn page_round(value: u64) -> Option<u64> {
    match value.checked_add(PAGE - 1) {
        Some(rounded) => Some(rounded & !(PAGE - 1)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KERNEL_END: u64 = KERNEL_START + 0x10_0000;

    fn handoff() -> BootHandoff {
        BootHandoff::qemu_default(0x0200_0000, 4)
    }

    #[test]
    fn validates_qemu_v3_handoff() {
        let h = handoff();
        assert!(h.validate(KERNEL_START, KERNEL_END));
        assert_eq!(h.effective_max_cpus(), 4);
        assert_eq!(h.words().len(), BootHandoff::WORDS);
        assert_eq!(h.words()[0], HANDOFF_MAGIC);
        assert_eq!(h.words()[8], 4);
        assert_eq!(h.words()[9], 0);
        assert_eq!(h.version, BOOT_HANDOFF_EVIDENCE_VERSION);
        assert_eq!(h.boot_evidence_addr, BOOT_EVIDENCE_ADDR);
        assert_eq!(h.boot_evidence_len, BootEvidenceV1::BYTES as u64);
    }

    #[test]
    fn q35_large_bootfs_avoids_replacement_workspace() {
        let compact = BootHandoff::qemu_x86_64(0x0800_0000, 4);
        assert_eq!(compact.bootfs_addr, 0x0800_0000);
        assert!(compact.validate(0x0200_0000, 0x0300_0000));
        let large = BootHandoff::qemu_x86_64(0x0800_1000, 4);
        assert!(large.bootfs_addr >= large.update_base + large.update_len);
        assert!(large.validate(0x0200_0000, 0x0300_0000));
        let oversized = BootHandoff::qemu_x86_64(0x2000_0000, 4);
        assert!(!oversized.validate(0x0200_0000, 0x0300_0000));
    }

    #[test]
    fn validates_optional_entropy_seed() {
        let mut h = handoff();
        h.entropy_seed_valid = 1;
        h.entropy_seed = [1, 2, 3, 4];
        assert!(h.validate(KERNEL_START, KERNEL_END));

        h.entropy_seed = [0; 4];
        assert!(!h.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let mut bad = handoff();
        bad.magic = 0;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));

        let mut bad = handoff();
        bad.version = BOOT_HANDOFF_VERSION + 1;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn rejects_invalid_cpu_counts() {
        let mut bad = handoff();
        bad.max_cpus = 0;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));

        bad.max_cpus = MAX_BOOT_CPUS + 1;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn rejects_kernel_bootfs_and_update_overlaps() {
        let mut bad = handoff();
        bad.bootfs_addr = KERNEL_START;
        bad.bootfs_len = PAGE;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));

        let mut bad = handoff();
        bad.update_base = KERNEL_START;
        bad.update_len = PAGE;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));

        let mut bad = handoff();
        bad.update_base = BOOTFS_ADDR;
        bad.update_len = PAGE;
        assert!(!bad.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn version_one_defaults_to_one_cpu() {
        let mut h = handoff();
        h.version = 1;
        h.max_cpus = 0;
        assert_eq!(h.effective_max_cpus(), 1);
        assert!(h.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn v3_requires_bounded_evidence_region() {
        let mut h = handoff();
        h.boot_evidence_len = 0;
        assert!(!h.validate(KERNEL_START, KERNEL_END));

        let mut h = handoff();
        h.boot_evidence_addr = BOOTFS_ADDR;
        assert!(!h.validate(KERNEL_START, KERNEL_END));

        let mut h = handoff();
        h.version = BOOT_HANDOFF_LEGACY_VERSION;
        h.boot_evidence_len = 0;
        assert!(h.validate(KERNEL_START, KERNEL_END));
    }

    #[test]
    fn boot_evidence_round_trips_and_rejects_malformed_records() {
        let mut evidence = BootEvidenceV1::qemu_dev(
            1,
            BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
                | BOOT_EVIDENCE_FLAG_IOMMU_STRICT,
            [1; 32],
            [2; 32],
            [3; 32],
            [4; 32],
        );
        evidence.signature = [9; 64];
        let mut bytes = [0; BootEvidenceV1::BYTES];
        evidence.encode(&mut bytes);
        assert_eq!(BootEvidenceV1::decode(&bytes), Some(evidence));

        bytes[0] = 0;
        assert!(BootEvidenceV1::decode(&bytes).is_none());

        let development = BootEvidenceV1::qemu_dev(
            1,
            BOOT_EVIDENCE_FLAG_IOMMU_STRICT,
            [1; 32],
            [2; 32],
            [3; 32],
            [4; 32],
        );
        let mut bytes = [0; BootEvidenceV1::BYTES];
        development.encode(&mut bytes);
        assert_eq!(BootEvidenceV1::decode(&bytes), Some(development));

        let verified = BootEvidenceV1::verified_bl33(
            2,
            BOOT_EVIDENCE_FLAG_SECURE_BOOT | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
            [1; 32],
            [2; 32],
            [3; 32],
            [4; 32],
            [5; 32],
        );
        verified.encode(&mut bytes);
        assert_eq!(BootEvidenceV1::decode(&bytes), Some(verified));
        let mut missing_root = verified;
        missing_root.public_key = [0; 32];
        missing_root.encode(&mut bytes);
        assert!(BootEvidenceV1::decode(&bytes).is_none());
    }
}
