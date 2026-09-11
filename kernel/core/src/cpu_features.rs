#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsidSupport {
    None,
    Bits8,
    Bits16,
}

impl AsidSupport {
    pub const fn max_asid(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Bits8 => 0xff,
            Self::Bits16 => u16::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aarch64CpuFeatures {
    pub asid: AsidSupport,
    pub fp_simd: bool,
    pub pan: bool,
    pub bti: bool,
    pub pointer_auth: bool,
    pub pac_address: bool,
    pub pac_generic: bool,
    pub speculation_barrier: bool,
    pub ssbs: bool,
    pub csv2: bool,
    pub csv3: bool,
    pub rndr: bool,
    pub timer_frequency_hz: u64,
}

impl Aarch64CpuFeatures {
    pub const fn minimal(timer_frequency_hz: u64) -> Self {
        Self {
            asid: AsidSupport::None,
            fp_simd: false,
            pan: false,
            bti: false,
            pointer_auth: false,
            pac_address: false,
            pac_generic: false,
            speculation_barrier: false,
            ssbs: false,
            csv2: false,
            csv3: false,
            rndr: false,
            timer_frequency_hz,
        }
    }

    pub const fn from_registers(
        mmfr0_el1: u64,
        pfr0_el1: u64,
        pfr1_el1: u64,
        isar0_el1: u64,
        isar1_el1: u64,
        _isar2_el1: u64,
        timer_frequency_hz: u64,
    ) -> Self {
        let asid_field = (mmfr0_el1 >> 4) & 0xf;
        let pac_address = ((isar1_el1 >> 4) & 0xf) != 0;
        let pac_generic = ((isar1_el1 >> 8) & 0xf) != 0;
        let sb = ((isar1_el1 >> 36) & 0xf) != 0;
        let ssbs = ((pfr1_el1 >> 4) & 0xf) != 0;
        Self {
            asid: if asid_field >= 2 {
                AsidSupport::Bits16
            } else if asid_field >= 1 {
                AsidSupport::Bits8
            } else {
                AsidSupport::None
            },
            fp_simd: ((pfr0_el1 >> 16) & 0xf) != 0xf && ((pfr0_el1 >> 20) & 0xf) != 0xf,
            pan: ((mmfr0_el1 >> 20) & 0xf) != 0,
            bti: ((pfr1_el1 >> 0) & 0xf) != 0,
            pointer_auth: pac_address || pac_generic,
            pac_address,
            pac_generic,
            speculation_barrier: sb,
            ssbs,
            csv2: ((pfr0_el1 >> 56) & 0xf) != 0,
            csv3: ((pfr0_el1 >> 60) & 0xf) != 0,
            rndr: ((isar0_el1 >> 60) & 0xf) != 0,
            timer_frequency_hz,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompiledHardening {
    pub bti: bool,
    pub pac_ret: bool,
    pub speculation_barriers: bool,
}

impl CompiledHardening {
    pub const fn product_default() -> Self {
        Self {
            bti: true,
            pac_ret: true,
            speculation_barriers: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelFeatureState {
    pub detected: Aarch64CpuFeatures,
    pub compiled: CompiledHardening,
    pub entropy_ready: bool,
    pub fp_simd_enabled: bool,
    pub pan_enabled: bool,
    pub asids_enabled: bool,
    pub bti_active: bool,
    pub pointer_auth_active: bool,
    pub speculation_controls_active: bool,
    pub bti_enabled: bool,
    pub pointer_auth_enabled: bool,
    pub speculation_controls_enabled: bool,
}

impl KernelFeatureState {
    pub const fn from_detected(detected: Aarch64CpuFeatures) -> Self {
        Self::policy(
            detected,
            CompiledHardening::product_default(),
            detected.rndr,
        )
    }

    pub const fn policy(
        detected: Aarch64CpuFeatures,
        compiled: CompiledHardening,
        entropy_ready: bool,
    ) -> Self {
        let bti_active = detected.bti && compiled.bti;
        let pointer_auth_active = detected.pointer_auth && compiled.pac_ret && entropy_ready;
        let speculation_controls_active =
            compiled.speculation_barriers && (detected.speculation_barrier || detected.ssbs);
        Self {
            detected,
            compiled,
            entropy_ready,
            fp_simd_enabled: detected.fp_simd,
            pan_enabled: detected.pan,
            asids_enabled: !matches!(detected.asid, AsidSupport::None),
            bti_active,
            pointer_auth_active,
            speculation_controls_active,
            bti_enabled: bti_active,
            pointer_auth_enabled: pointer_auth_active,
            speculation_controls_enabled: speculation_controls_active,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AsidAllocator {
    next: u16,
    max: u16,
}

impl AsidAllocator {
    pub const fn disabled() -> Self {
        Self { next: 0, max: 0 }
    }

    pub const fn new(support: AsidSupport) -> Self {
        Self {
            next: if matches!(support, AsidSupport::None) {
                0
            } else {
                1
            },
            max: support.max_asid(),
        }
    }

    pub const fn enabled(&self) -> bool {
        self.max != 0
    }

    pub const fn restore(next: u16, max: u16) -> Self {
        Self { next, max }
    }

    pub const fn next_asid(&self) -> u16 {
        self.next
    }

    pub const fn max_asid(&self) -> u16 {
        self.max
    }

    pub fn allocate(&mut self) -> Option<u16> {
        if !self.enabled() {
            return Some(0);
        }
        let asid = self.next;
        if asid == 0 {
            return None;
        }
        self.next = if self.next == self.max {
            1
        } else {
            self.next + 1
        };
        Some(asid)
    }
}
