//! CPU0-owned QEMU takeover with an independent replacement stack, heap and runtime.
use crate::arch::ArchAPI;
pub(crate) use crate::arch::cpu_transplant as cpu;
mod image;
mod live;
mod orchestrator;
pub mod prepare;
use bexos_boot::{PAGE, UPDATE_SNAPSHOT};
use bexos_kernel_core::runtime::Backend;
use bexos_kernel_core::runtime::Context;
use bexos_kernel_core::transplant::{
    Aarch64CpuContextRecord, HANDOFF_MAGIC, HANDOFF_VERSION, KernelRange, KernelTransplantHandoff,
    codec::Reader,
};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
pub use live::step as live_step;
const HANDOFF_ADDR: u64 = bexos_boot::HANDOFF_ADDR + PAGE;
static UPDATE_STATUS: AtomicU8 = AtomicU8::new(1);
static UPDATE_GENERATION: AtomicU64 = AtomicU64::new(0);
static PENDING: AtomicU64 = AtomicU64::new(0);
pub fn is_pending() -> bool {
    PENDING.load(Ordering::Acquire) != 0
}
unsafe extern "C" {
    static __kernel_start: u8;
}

pub fn stage_transplant(
    rt: &mut crate::syscall::Rt,
    generation: u64,
    target: &str,
    artifact_hash: [u8; 32],
    artifact: &[u8],
) -> Result<&'static str, &'static str> {
    if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE) {
        return Err("debugd privilege required");
    }
    if PENDING.load(Ordering::Acquire) != 0 || rt.handover.is_some() {
        return Err("update already staged");
    }
    if generation <= UPDATE_GENERATION.load(Ordering::Acquire) {
        return Err("rollback generation");
    }
    if target
        != if cfg!(target_arch = "x86_64") {
            "qemu-x86_64-kernel"
        } else {
            "qemu-aarch64-kernel"
        }
    {
        return Err("unsupported platform target");
    }
    if crate::arch::CurrentArch::active_cpu_mask()
        != crate::arch::CurrentArch::configured_cpu_mask()
    {
        return Err("secondary CPUs have not acknowledged boot readiness");
    }
    if *blake3::hash(artifact).as_bytes() != artifact_hash {
        return Err("artifact hash mismatch");
    }
    let old_start = core::ptr::addr_of!(__kernel_start) as u64;
    // All old heap allocations can be reclaimed: no native pointers cross the ABI.
    let old_kernel = KernelRange::new(
        old_start,
        crate::memory::kernel_end() + crate::memory::HEAP_BYTES as u64 - old_start,
    );
    let (plan, replacement) = image::validate(artifact, old_kernel)?;
    let mut handoff = KernelTransplantHandoff {
        magic: HANDOFF_MAGIC,
        version: HANDOFF_VERSION,
        architecture: bexos_kernel_core::runtime::Context::ARCHITECTURE,
        architecture_state: crate::arch::CurrentArch::capture_architecture_state(),
        generation,
        old_kernel,
        old_reclaim: old_kernel,
        replacement,
        handoff: KernelRange::new(HANDOFF_ADDR, PAGE),
        snapshot: KernelRange::new(UPDATE_SNAPSHOT, 1),
        replacement_entry: plan.entry_vaddr,
        cpu: capture_cpu_context(),
        system: crate::arch::CurrentArch::capture_system(),
        switch_authorized: 0,
        artifact_hash,
        snapshot_checksum: 0,
        checksum: 0,
    };
    handoff.seal();
    handoff.validate().map_err(|_| "invalid handoff ranges")?;
    crate::arch::CurrentArch::park_secondaries(rt)?;
    image::load(&plan, artifact);
    crate::arch::CurrentArch::stage_replacement(rt, &plan)
        .map_err(|_| "replacement execution mappings rejected")?;
    unsafe {
        core::ptr::write(HANDOFF_ADDR as *mut KernelTransplantHandoff, handoff);
    }
    UPDATE_STATUS.store(2, Ordering::Release);
    PENDING.store(HANDOFF_ADDR, Ordering::Release);
    crate::log_line("heart-transplant: full replacement kernel staged");
    Ok("kernel platform update staged")
}

pub fn commit_pending(frame: *mut Context) {
    if PENDING.load(Ordering::Acquire) == 0 {
        return;
    }
    let allowed = crate::userspace::RUNTIME.with(|slot| {
        let rt = slot.as_mut().unwrap();
        if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE) {
            return false;
        }
        rt.save_context(unsafe { *frame });
        true
    });
    if !allowed {
        return;
    }
    if live::begin().is_err() {
        live::abort();
    }
}

// Assembly establishes the replacement stack before calling Rust.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_transplant_main(handoff_ptr: u64) -> ! {
    crate::state::UART.with(|u| *u = Some(crate::arch::CurrentArch::early_console()));
    crate::arch::CurrentArch::install_exception_vectors();
    assert_eq!(handoff_ptr, HANDOFF_ADDR);
    let handoff = unsafe { *(handoff_ptr as *const KernelTransplantHandoff) };
    handoff.validate().expect("replacement handoff");
    assert_eq!(handoff.switch_authorized, 1);
    let own_start = core::ptr::addr_of!(__kernel_start) as u64;
    assert_eq!(handoff.replacement.start, own_start, "replacement identity");
    assert!(handoff.replacement.contains(KernelRange::new(
        own_start,
        crate::memory::kernel_end() - own_start
    )));
    let bytes = unsafe {
        core::slice::from_raw_parts(
            handoff.snapshot.start as *const u8,
            handoff.snapshot.len as usize,
        )
    };
    handoff
        .validate_snapshot(bytes)
        .expect("replacement snapshot checksum");
    crate::log_line("heart-transplant: replacement kernel entered");
    let mut reader = Reader::new(&bytes[32..]);
    assert_eq!(reader.word().unwrap(), prepare::RECEIPT_MAGIC);
    assert_eq!(reader.word().unwrap(), handoff.generation);
    let sequence = reader.word().unwrap();
    let digest = reader.word().unwrap();
    let mut rt = alloc::boxed::Box::new(
        prepare::take(handoff.generation, sequence, digest).expect("prepared runtime receipt"),
    );
    let process = &rt.processes[rt.current];
    let thread = &rt.threads[rt.current_thread];
    assert_eq!(thread.process, rt.current, "replacement current thread");
    let context = thread.context;
    let root = process.root;
    let asid = process.asid;
    assert!(crate::arch::CurrentArch::matches_handoff_address_space(
        &handoff.cpu,
        root,
        asid
    ));
    crate::arch::CurrentArch::restore_system(&handoff.system);
    if handoff.version >= 4 {
        crate::arch::CurrentArch::restore_architecture_state(&handoff.architecture_state);
    }
    crate::arch::CurrentArch::switch_address_space(
        bexos_kernel_core::runtime::AddressSpaceSwitch {
            root_table_phys: root,
            asid,
            userspace_pac_key: process.userspace_pac_key,
        },
    );
    orchestrator::complete(&handoff);
    UPDATE_GENERATION.store(handoff.generation, Ordering::Release);
    UPDATE_STATUS.store(4, Ordering::Release);
    // All old stack and heap references are gone before any page becomes free.
    crate::arch::CurrentArch::reclaim_old_kernel(&mut rt, handoff.old_reclaim);
    let before = rt.backend.free_pages();
    rt.backend
        .release(handoff.old_reclaim.start, handoff.old_reclaim.len / PAGE);
    assert_eq!(
        rt.backend.free_pages(),
        before + handoff.old_reclaim.len / PAGE
    );
    let probe = rt.backend.allocate(1).expect("old kernel allocation probe");
    assert!(handoff.old_reclaim.contains(KernelRange::new(probe, PAGE)));
    rt.backend
        .write(probe, b"replacement owns this former kernel page");
    rt.backend.release(probe, 1);
    crate::log_line("heart-transplant: old kernel reclaimed; allocation from old range verified");
    crate::userspace::RUNTIME.with(|slot| *slot = Some(rt));
    crate::arch::CurrentArch::set_timer_interval(crate::arch::CurrentArch::timer_frequency() / 10);
    crate::log_line("heart-transplant: takeover complete; resuming preserved EL0 task");
    unsafe { crate::arch::CurrentArch::enter_context(&context) }
}

pub fn update_status() -> (u8, u64, &'static str) {
    let status = UPDATE_STATUS.load(Ordering::Acquire);
    (
        status,
        UPDATE_GENERATION.load(Ordering::Acquire),
        match status {
            1 => "idle",
            2 => match live::phase() {
                Some(bexos_migration::Phase::Bulk) => "live bulk sync; userspace serving",
                Some(_) => "catching up live mutations; userspace serving",
                None => "replacement kernel staged",
            },
            3 => "applying transplant",
            4 => "replacement kernel completed; old memory reclaimed",
            _ => "kernel platform update failed",
        },
    )
}
fn capture_cpu_context() -> Aarch64CpuContextRecord {
    crate::arch::CurrentArch::capture_cpu_context()
}
