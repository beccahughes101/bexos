use super::{acpi, context, interrupts, time};
use bexos_kernel_core::cpu_features::{Aarch64CpuFeatures, KernelFeatureState};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
static ECAM: AtomicU64 = AtomicU64::new(0);
pub fn pci_ecam_base() -> u64 {
    ECAM.load(Ordering::Acquire)
}
static BOOTED: AtomicU64 = AtomicU64::new(1);
static CONFIGURED: AtomicU64 = AtomicU64::new(1);
static RELEASED: AtomicBool = AtomicBool::new(false);
static IDS: [AtomicU8; 64] = [const { AtomicU8::new(0) }; 64];
pub fn apic_id(cpu: usize) -> u8 {
    IDS[cpu].load(Ordering::Acquire)
}
pub fn current_cpu_id() -> u64 {
    let id = unsafe { core::arch::x86_64::__cpuid(1) }.ebx >> 24;
    (0..CONFIGURED.load(Ordering::Acquire) as usize)
        .find(|i| apic_id(*i) as u32 == id)
        .unwrap_or(0) as u64
}
pub fn initialize_primary(_features: KernelFeatureState, _seed: [u64; 4]) {
    super::transplant::initialize_parking_trampoline();
    let tables = acpi::discover();
    ECAM.store(tables.ecam, Ordering::Release);
    let configured = CONFIGURED.load(Ordering::Acquire) as usize;
    assert!(configured <= tables.count && configured <= 64);
    let bsp = (unsafe { core::arch::x86_64::__cpuid(1) }.ebx >> 24) as u8;
    IDS[0].store(bsp, Ordering::Release);
    let mut next = 1;
    for id in &tables.ids[..tables.count] {
        if *id != bsp && next < configured {
            IDS[next].store(*id, Ordering::Release);
            next += 1;
        }
    }
    context::install_exception_vectors();
    interrupts::discover(tables.lapic, tables.ioapic);
    time::initialize(tables.hpet);
}
pub fn initialize_secondary(_features: KernelFeatureState) {
    context::install_exception_vectors();
}
pub fn detect_cpu_features() -> KernelFeatureState {
    let mut f = Aarch64CpuFeatures::minimal(time::timer_frequency());
    f.fp_simd = true;
    KernelFeatureState::policy(
        f,
        bexos_kernel_core::cpu_features::CompiledHardening {
            bti: false,
            pac_ret: false,
            speculation_barriers: true,
        },
        false,
    )
}
pub fn active_cpu_mask() -> u64 {
    BOOTED.load(Ordering::Acquire)
}
pub fn expected_cpu_mask(count: u32) -> u64 {
    if count == 64 {
        u64::MAX
    } else {
        (1u64 << count) - 1
    }
}
pub fn set_configured_max_cpus(count: u32) {
    assert!((1..=64).contains(&count));
    CONFIGURED.store(count as u64, Ordering::Release);
}
pub fn configured_cpu_mask() -> u64 {
    expected_cpu_mask(CONFIGURED.load(Ordering::Acquire) as u32)
}
pub fn release_secondary_schedulers() {
    RELEASED.store(true, Ordering::Release);
}
pub fn wait_for_scheduler_release() {
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
}
pub fn wait_for_cpu_boot_mask(expected: u64) -> u64 {
    let deadline = time::monotonic_ns() + 1_000_000_000;
    while active_cpu_mask() & expected != expected {
        assert!(time::monotonic_ns() < deadline, "AP startup timeout");
        core::hint::spin_loop();
    }
    active_cpu_mask()
}
#[cfg(not(bexos_update_kernel))]
pub fn initialize_smp(max_cpus: u32) {
    unsafe extern "C" {
        static __x86_ap_start: u8;
        static __x86_ap_end: u8;
        static __x86_boot_root: u8;
    }
    let start = core::ptr::addr_of!(__x86_ap_start);
    let len = core::ptr::addr_of!(__x86_ap_end) as usize - start as usize;
    assert!(len < 0xff0);
    unsafe {
        core::ptr::copy_nonoverlapping(start, 0x8000 as *mut u8, len);
        core::ptr::write_volatile(
            0x8ff0 as *mut u64,
            core::ptr::addr_of!(__x86_boot_root) as u64,
        );
    }
    for cpu in 1..max_cpus as usize {
        let id = apic_id(cpu);
        // A serialized mailbox assigns stacks by logical CPU, not sparse APIC IDs.
        unsafe {
            core::ptr::write_volatile(0x8ff8 as *mut u64, cpu as u64);
        }
        interrupts::send(id, 0xc500);
        time::delay(10_000_000);
        interrupts::send(id, 0x8500);
        time::delay(200_000);
        interrupts::send(id, 0x608);
        time::delay(200_000);
        interrupts::send(id, 0x608);
        wait_for_cpu_boot_mask(1u64 << cpu);
    }
}
#[cfg(bexos_update_kernel)]
pub fn initialize_smp(_max_cpus: u32) { /* Existing secondaries are retained/parked by takeover. */
}
#[unsafe(no_mangle)]
pub extern "C" fn x86_secondary_entry() -> ! {
    let cpu = current_cpu_id();
    context::install_exception_vectors();
    BOOTED.fetch_or(1 << cpu, Ordering::AcqRel);
    crate::secondary_cpu_main(cpu)
}

pub fn capture_state(state: &mut [u64]) {
    state[0] = CONFIGURED.load(Ordering::Acquire);
    state[1] = BOOTED.load(Ordering::Acquire);
    for i in 0..8 {
        state[2 + i] = u64::from_le_bytes(core::array::from_fn(|j| apic_id(i * 8 + j)));
    }
}
pub fn restore_state(state: &[u64]) {
    CONFIGURED.store(state[0], Ordering::Release);
    BOOTED.store(state[1], Ordering::Release);
    for i in 0..8 {
        for (j, id) in state[2 + i].to_le_bytes().into_iter().enumerate() {
            IDS[i * 8 + j].store(id, Ordering::Release);
        }
    }
    RELEASED.store(true, Ordering::Release);
}

pub fn restore_ecam(base: u64) {
    ECAM.store(base, Ordering::Release);
}
