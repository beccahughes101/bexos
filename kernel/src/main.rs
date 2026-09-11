#![no_main]
#![no_std]
extern crate alloc;
use crate::arch::ArchAPI;
mod arch;
use arch::early_uart;
use arch::interrupts;
mod memory;
use arch::mmu;
mod panic;
mod sched;
#[cfg(target_arch = "x86_64")]
mod secure_memory;
mod state;
mod syscall;
mod tracing;
mod transplant;
mod trusty;
mod userspace;
use core::fmt::Write;

use bexos_boot::{
    BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK, BOOT_EVIDENCE_FLAG_SECURE_BOOT,
    BOOT_EVIDENCE_VERIFIED_BL33_VERSION, BootEvidenceV1, BootHandoff, HANDOFF_ADDR, RAM_END,
    RAM_START,
};
use bexos_crypto::verify_ed25519;
use sha2::{Digest, Sha256};

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(handoff_ptr: u64) -> ! {
    state::UART.with(|s| {
        let mut u = arch::CurrentArch::early_console();
        u.init();
        *s = Some(u);
    });
    log_line("kernel: boot kernel_main");
    log_line("kernel: early console ready");
    let handoff = boot_handoff(handoff_ptr);
    let (secure_boot, evidence_flags) = validate_boot_evidence(&handoff);
    arch::CurrentArch::configure_secure_monitor(
        evidence_flags,
        handoff.secure_monitor_call_header,
        handoff.secure_monitor_features,
    );
    let raw_max_cpus = handoff.effective_max_cpus();
    let max_cpus = if raw_max_cpus <= u32::MAX as u64 {
        raw_max_cpus as u32
    } else {
        1
    };
    arch::CurrentArch::set_configured_max_cpus(max_cpus);
    let detected = arch::CurrentArch::detect_cpu_features();
    let pac_seed = if handoff.entropy_seed_valid == 1 {
        Some(handoff.entropy_seed)
    } else if detected.detected.rndr {
        arch::CurrentArch::random_seed()
    } else {
        None
    };
    let features = bexos_kernel_core::cpu_features::KernelFeatureState::policy(
        detected.detected,
        detected.compiled,
        pac_seed.is_some(),
    );
    arch::CurrentArch::enable_user_access_protection(features);
    arch::CurrentArch::initialize_primary(features, pac_seed.unwrap_or([0; 4]));
    arch::CurrentArch::quiesce_firmware_irqs();
    let trusty_available = trusty::prepare_bootstrap(secure_boot, max_cpus);
    // Complete primary secure initialization before secondary entry can run
    // secure applications concurrently with initialization of IRQ handling.
    trusty::finish_bootstrap(trusty_available);
    arch::CurrentArch::initialize_smp(max_cpus);
    // Secure startup can hold the secondary CPUs until its bootstrap work is
    // serviced on CPU0. Finish it before waiting for normal-world readiness.
    trusty::finish_bootstrap(trusty_available);
    let expected_mask = arch::CurrentArch::expected_cpu_mask(max_cpus);
    let cpus = arch::CurrentArch::wait_for_cpu_boot_mask(expected_mask);
    #[cfg(target_arch = "aarch64")]
    trusty::register_interrupts(trusty_available);
    for cpu in 0..max_cpus {
        if cpus & (1 << cpu) != 0 {
            state::UART.with(|s| {
                if let Some(u) = s.as_mut() {
                    let _ = writeln!(u, "kernel: cpu{cpu} boot path ready");
                }
            });
        }
    }
    let first = memory::init(&handoff);
    userspace::init(first, features, handoff, pac_seed);
    log_line("kernel: userspace init complete");
    arch::CurrentArch::initialize_interrupts(max_cpus);
    log_line("kernel: interrupts init complete");
    arch::CurrentArch::release_secondary_schedulers();
    log_line("kernel: smp scheduler enabled");
    userspace::launch()
}

fn validate_boot_evidence(handoff: &BootHandoff) -> (bool, u64) {
    if handoff.version < 3 {
        log_line("kernel: legacy non-secure boot handoff accepted");
        return (false, 0);
    }
    let bytes = unsafe {
        core::slice::from_raw_parts(
            handoff.boot_evidence_addr as *const u8,
            handoff.boot_evidence_len as usize,
        )
    };
    let evidence = BootEvidenceV1::decode(bytes).expect("valid boot evidence");
    #[cfg(target_arch = "x86_64")]
    if evidence.flags & BOOT_EVIDENCE_FLAG_SECURE_BOOT != 0 {
        assert_eq!(
            evidence.version,
            bexos_boot::BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION,
            "x86 requires resident boot evidence"
        );
        assert!(
            arch::x86_64::confirm_boot_evidence(
                Sha256::digest(&bytes[..BootEvidenceV1::BYTES]).into()
            ),
            "resident monitor rejected boot evidence"
        );
    }
    if matches!(
        evidence.version,
        BOOT_EVIDENCE_VERIFIED_BL33_VERSION | bexos_boot::BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION
    ) {
        assert_eq!(
            evidence.version == bexos_boot::BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION,
            cfg!(target_arch = "x86_64"),
            "boot evidence architecture"
        );
        let required = BOOT_EVIDENCE_FLAG_SECURE_BOOT;
        assert_eq!(
            evidence.flags & required,
            required,
            "authenticated BL33 flags"
        );
    } else {
        let mut signed = [0; BootEvidenceV1::SIGNED_BYTES];
        evidence.encode_unsigned(&mut signed);
        verify_ed25519(&evidence.public_key, &signed, &evidence.signature)
            .expect("boot evidence signature");
    }
    let bootfs = unsafe {
        core::slice::from_raw_parts(
            handoff.bootfs_addr as *const u8,
            handoff.bootfs_len as usize,
        )
    };
    let bootfs_sha256: [u8; 32] = Sha256::digest(bootfs).into();
    assert_eq!(bootfs_sha256, evidence.bootfs_sha256, "BootFS SHA-256");
    if evidence.flags & BOOT_EVIDENCE_FLAG_SECURE_BOOT != 0 {
        log_line("kernel: secure boot evidence verified");
    } else {
        log_line("kernel: development boot evidence verified; secure boot inactive");
    }
    if evidence.flags & BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK != 0 {
        log_line("kernel: RPMB anti-rollback backend verified");
    }
    (
        evidence.flags & BOOT_EVIDENCE_FLAG_SECURE_BOOT != 0,
        evidence.flags,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn secondary_cpu_main(cpu_id: u64) -> ! {
    arch::CurrentArch::wait_for_scheduler_release();
    let features = arch::CurrentArch::detect_cpu_features();
    arch::CurrentArch::enable_user_access_protection(features);
    arch::CurrentArch::initialize_secondary(features);
    arch::CurrentArch::initialize_secondary_interrupts();
    state::UART.with(|s| {
        if let Some(u) = s.as_mut() {
            let _ = writeln!(u, "kernel: cpu{cpu_id} scheduler idle ready");
        }
    });
    loop {
        arch::CurrentArch::wait_for_interrupt();
    }
}

unsafe extern "C" {
    static __kernel_start: u8;
}

fn boot_handoff(handoff_ptr: u64) -> BootHandoff {
    let kernel_start = core::ptr::addr_of!(__kernel_start) as u64;
    let kernel_end = memory::kernel_end();
    if let Some(handoff) = read_valid_handoff(handoff_ptr, kernel_start, kernel_end) {
        log_line("kernel: boot handoff accepted from x0");
        return handoff;
    }
    if let Some(mut handoff) = read_valid_handoff(HANDOFF_ADDR, kernel_start, kernel_end) {
        // Raw AArch64 QEMU entry supplies a DTB in x0 and the handoff separately.
        #[cfg(target_arch = "aarch64")]
        if !handoff.framebuffer.valid()
            && handoff_ptr >= RAM_START
            && handoff_ptr
                .checked_add(40)
                .is_some_and(|end| end <= RAM_END)
        {
            let header = unsafe { core::slice::from_raw_parts(handoff_ptr as *const u8, 40) };
            let length = u32::from_be_bytes(header[4..8].try_into().unwrap()) as u64;
            if header[..4] == [0xd0, 0x0d, 0xfe, 0xed]
                && (40..=2 * 1024 * 1024).contains(&length)
                && handoff_ptr
                    .checked_add(length)
                    .is_some_and(|end| end <= RAM_END)
            {
                let dtb = unsafe {
                    core::slice::from_raw_parts(handoff_ptr as *const u8, length as usize)
                };
                if let Some(framebuffer) = bexos_boot::framebuffer::simple_framebuffer(dtb) {
                    handoff.framebuffer = framebuffer;
                    validate_framebuffer(&mut handoff, kernel_start, kernel_end);
                }
            }
        }
        log_line("kernel: boot handoff accepted from fixed address");
        return handoff;
    }
    panic!("invalid boot handoff");
}

fn read_valid_handoff(ptr: u64, kernel_start: u64, kernel_end: u64) -> Option<BootHandoff> {
    if ptr < RAM_START
        || ptr.checked_add(core::mem::size_of::<BootHandoff>() as u64)? > RAM_END
        || ptr % core::mem::align_of::<BootHandoff>() as u64 != 0
    {
        return None;
    }
    let mut handoff = unsafe { core::ptr::read(ptr as *const BootHandoff) };
    handoff.normalize_legacy_extensions();
    if !handoff.framebuffer.valid() {
        handoff.framebuffer = bexos_boot::BootFramebuffer::NONE;
    }
    validate_framebuffer(&mut handoff, kernel_start, kernel_end);
    handoff
        .validate(kernel_start, kernel_end)
        .then_some(handoff)
}
fn validate_framebuffer(handoff: &mut BootHandoff, kernel_start: u64, kernel_end: u64) {
    let f = handoff.framebuffer;
    if f.valid()
        && [
            (
                kernel_start,
                kernel_end.saturating_add(crate::memory::HEAP_BYTES as u64),
            ),
            (
                handoff.bootfs_addr,
                handoff.bootfs_addr.saturating_add(handoff.bootfs_len),
            ),
            (
                handoff.update_base,
                handoff.update_base.saturating_add(handoff.update_len),
            ),
            (
                handoff.boot_evidence_addr,
                handoff.boot_evidence_addr.saturating_add(4096),
            ),
        ]
        .iter()
        .any(|(a, b)| f.address < *b && f.address + f.length > *a)
    {
        handoff.framebuffer = bexos_boot::BootFramebuffer::NONE;
    }
}

pub fn log_line(message: &str) {
    state::UART.with(|s| {
        if let Some(u) = s.as_mut() {
            let _ = writeln!(u, "{message}");
        }
    });
}
mod migration;
