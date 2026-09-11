use bexos_kernel_core::cpu_features::KernelFeatureState;
use bexos_kernel_core::runtime::PacKeyMaterial;
use core::sync::atomic::{AtomicU64, Ordering};

static ACTIVE_POLICY: AtomicU64 = AtomicU64::new(0);
static KERNEL_APIB_LO: AtomicU64 = AtomicU64::new(0);
static KERNEL_APIB_HI: AtomicU64 = AtomicU64::new(0);
const POLICY_BTI_ACTIVE: u64 = 1 << 0;
const POLICY_PAC_ACTIVE: u64 = 1 << 1;
const POLICY_SPEC_ACTIVE: u64 = 1 << 2;
const POLICY_SB_DETECTED: u64 = 1 << 3;
const POLICY_SSBS_DETECTED: u64 = 1 << 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeculationBarrier {
    Sb,
    DsbIsb,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacKeys {
    pub apiakeylo: u64,
    pub apiakeyhi: u64,
    pub apibkeylo: u64,
    pub apibkeyhi: u64,
}

impl PacKeys {
    pub const fn from_seed(seed: [u64; 4]) -> Self {
        Self {
            apiakeylo: seed[0],
            apiakeyhi: seed[1],
            apibkeylo: seed[2],
            apibkeyhi: seed[3],
        }
    }
}

pub fn select_speculation_barrier(features: KernelFeatureState) -> SpeculationBarrier {
    if features.detected.speculation_barrier {
        SpeculationBarrier::Sb
    } else {
        SpeculationBarrier::DsbIsb
    }
}

pub fn initialize_primary(features: KernelFeatureState, seed: [u64; 4]) {
    ACTIVE_POLICY.store(pack_policy(features), Ordering::Release);
    let keys = PacKeys::from_seed(seed);
    KERNEL_APIB_LO.store(keys.apibkeylo, Ordering::Release);
    KERNEL_APIB_HI.store(keys.apibkeyhi, Ordering::Release);
    load_kernel_pac_keys(features, keys);
    configure_sctlr(features);
    configure_ssbs(features);
    speculation_barrier(select_speculation_barrier(features));
}

pub fn initialize_secondary(features: KernelFeatureState) {
    let primary = ACTIVE_POLICY.load(Ordering::Acquire);
    let policy = if primary == 0 {
        pack_policy(features)
    } else {
        primary
    };
    let keys = PacKeys {
        apiakeylo: 0,
        apiakeyhi: 0,
        apibkeylo: KERNEL_APIB_LO.load(Ordering::Acquire),
        apibkeyhi: KERNEL_APIB_HI.load(Ordering::Acquire),
    };
    load_kernel_pac_keys(features, keys);
    configure_sctlr_policy(policy);
    configure_ssbs_policy(policy);
    speculation_barrier(if policy & POLICY_SB_DETECTED != 0 {
        SpeculationBarrier::Sb
    } else {
        SpeculationBarrier::DsbIsb
    });
}

pub fn read_rndr_seed() -> Option<[u64; 4]> {
    let mut seed = [0; 4];
    for word in &mut seed {
        unsafe {
            core::arch::asm!(
                "mrs {out:x}, S3_3_C2_C4_0",
                out = out(reg) * word,
                options(nostack)
            );
        }
    }
    seed.iter().any(|word| *word != 0).then_some(seed)
}

pub fn load_userspace_pac_keys(features: KernelFeatureState, keys: PacKeys) {
    if !features.pointer_auth_active {
        return;
    }
    unsafe {
        core::arch::asm!(
            "msr S3_0_C2_C1_0, {apiakeylo:x}",
            "msr S3_0_C2_C1_1, {apiakeyhi:x}",
            apiakeylo = in(reg) keys.apiakeylo,
            apiakeyhi = in(reg) keys.apiakeyhi,
            options(nostack)
        );
    }
}

pub fn load_userspace_pac_key_for_switch(key: PacKeyMaterial) {
    if ACTIVE_POLICY.load(Ordering::Acquire) & POLICY_PAC_ACTIVE == 0 {
        return;
    }
    unsafe {
        core::arch::asm!(
            "msr S3_0_C2_C1_0, {apiakeylo:x}",
            "msr S3_0_C2_C1_1, {apiakeyhi:x}",
            apiakeylo = in(reg) key.lo,
            apiakeyhi = in(reg) key.hi,
            options(nostack)
        );
    }
}

pub fn load_kernel_pac_keys(features: KernelFeatureState, keys: PacKeys) {
    if !features.pointer_auth_active {
        return;
    }
    unsafe {
        core::arch::asm!(
            "msr S3_0_C2_C1_2, {apibkeylo:x}",
            "msr S3_0_C2_C1_3, {apibkeyhi:x}",
            apibkeylo = in(reg) keys.apibkeylo,
            apibkeyhi = in(reg) keys.apibkeyhi,
            options(nostack)
        );
    }
}

pub fn speculation_barrier(kind: SpeculationBarrier) {
    unsafe {
        match kind {
            SpeculationBarrier::Sb => core::arch::asm!("hint #18", options(nostack)),
            SpeculationBarrier::DsbIsb => core::arch::asm!("dsb nsh; isb", options(nostack)),
        }
    }
}

fn configure_sctlr(features: KernelFeatureState) {
    configure_sctlr_policy(pack_policy(features));
}

fn configure_sctlr_policy(policy: u64) {
    unsafe {
        let mut sctlr: u64;
        core::arch::asm!("mrs {sctlr:x}, sctlr_el1", sctlr = out(reg) sctlr, options(nostack));
        sctlr |= 1 << 19;
        if policy & POLICY_BTI_ACTIVE != 0 {
            sctlr |= 1 << 13;
        }
        core::arch::asm!("msr sctlr_el1, {sctlr:x}", sctlr = in(reg) sctlr, options(nostack));
        core::arch::asm!("isb", options(nostack));
    }
}

fn configure_ssbs(features: KernelFeatureState) {
    configure_ssbs_policy(pack_policy(features));
}

fn configure_ssbs_policy(policy: u64) {
    if policy & POLICY_SSBS_DETECTED == 0 {
        return;
    }
    unsafe {
        core::arch::asm!("msr S3_3_C4_C2_6, {enabled:x}", enabled = in(reg) 1_u64, options(nostack));
    }
}

fn pack_policy(features: KernelFeatureState) -> u64 {
    (if features.bti_active {
        POLICY_BTI_ACTIVE
    } else {
        0
    }) | (if features.pointer_auth_active {
        POLICY_PAC_ACTIVE
    } else {
        0
    }) | (if features.speculation_controls_active {
        POLICY_SPEC_ACTIVE
    } else {
        0
    }) | (if features.detected.speculation_barrier {
        POLICY_SB_DETECTED
    } else {
        0
    }) | (if features.detected.ssbs {
        POLICY_SSBS_DETECTED
    } else {
        0
    })
}
