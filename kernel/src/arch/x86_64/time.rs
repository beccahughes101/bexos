use super::{interrupts, io};
use core::sync::atomic::{AtomicU64, Ordering};
static DEADLINES: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
pub fn timer_deadline() -> u64 {
    DEADLINES[super::cpu::current_cpu_id() as usize].load(Ordering::Acquire)
}
static HPET: AtomicU64 = AtomicU64::new(0);
static PERIOD: AtomicU64 = AtomicU64::new(0);
static APIC_HZ: AtomicU64 = AtomicU64::new(0);
pub fn initialize(address: u64) {
    unsafe {
        let caps = io::read64(address);
        let period = caps >> 32;
        assert!(period != 0 && caps & (1 << 13) != 0, "64-bit HPET required");
        HPET.store(address, Ordering::Release);
        PERIOD.store(period, Ordering::Release);
        io::write64(address + 0x10, 0);
        io::write64(address + 0xf0, 0);
        io::write64(address + 0x10, 1);
    }
    interrupts::write(0x3e0, 3); // Divide by 16.
    interrupts::write(0x320, (1 << 16) | 32);
    interrupts::write(0x380, u32::MAX);
    let start = monotonic_ns();
    while monotonic_ns() - start < 10_000_000 {
        core::hint::spin_loop();
    }
    let elapsed = u32::MAX - interrupts::read(0x390);
    let duration = monotonic_ns() - start;
    assert!(elapsed != 0);
    APIC_HZ.store(
        (elapsed as u128 * 1_000_000_000 / duration as u128) as u64,
        Ordering::Release,
    );
    disable_timer();
}
pub fn timer_frequency() -> u64 {
    1_000_000_000
}
pub fn timer_ticks() -> u64 {
    monotonic_ns()
}
pub fn counter_value() -> u64 {
    monotonic_ns()
}
pub fn monotonic_ns() -> u64 {
    let base = HPET.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    ((unsafe { io::read64(base + 0xf0) } as u128 * PERIOD.load(Ordering::Acquire) as u128)
        / 1_000_000) as u64
}
pub fn set_timer_interval(ticks: u64) {
    set_timer_deadline_ns(monotonic_ns().saturating_add(ticks));
}
pub fn set_timer_deadline_ns(deadline: u64) {
    DEADLINES[super::cpu::current_cpu_id() as usize].store(deadline, Ordering::Release);
    let ns = deadline.saturating_sub(monotonic_ns()).max(1);
    let count = ((ns as u128 * APIC_HZ.load(Ordering::Acquire) as u128).div_ceil(1_000_000_000))
        .clamp(1, u32::MAX as u128) as u32;
    interrupts::write(0x3e0, 3);
    interrupts::write(0x320, 32);
    interrupts::write(0x380, count);
}
pub fn disable_timer() {
    interrupts::write(0x320, (1 << 16) | 32);
    interrupts::write(0x380, 0);
}
pub fn delay(ns: u64) {
    let start = monotonic_ns();
    while monotonic_ns().saturating_sub(start) < ns {
        core::hint::spin_loop();
    }
}

pub fn capture_state() -> [u64; 3] {
    [
        HPET.load(Ordering::Acquire),
        PERIOD.load(Ordering::Acquire),
        APIC_HZ.load(Ordering::Acquire),
    ]
}
pub fn restore_state(state: &[u64]) {
    HPET.store(state[0], Ordering::Release);
    PERIOD.store(state[1], Ordering::Release);
    APIC_HZ.store(state[2], Ordering::Release);
}

/// Exercise an external Q35 interrupt route independently of LAPIC timers.
/// Timer 0 is reserved for this boot check; HPET's main counter keeps running.
pub fn verify_ioapic_route() {
    let base = HPET.load(Ordering::Acquire);
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    let capabilities = unsafe { io::read64(base + 0x100) };
    let routes = (capabilities >> 32) as u32;
    let gsi = (16..32)
        .find(|gsi| {
            routes & (1 << gsi) != 0
                && super::ioapic::route(*gsi, super::cpu::apic_id(0), false, false)
        })
        .expect("HPET IOAPIC route");
    let ticks = 1_000_000_000_000u64 / PERIOD.load(Ordering::Acquire); // 1 ms.
    unsafe {
        io::write64(base + 0x100, (u64::from(gsi) << 9) | (1 << 2));
        io::write64(
            base + 0x108,
            io::read64(base + 0xf0).wrapping_add(ticks.max(1)),
        );
        core::arch::asm!("sti", options(nomem, nostack));
    }
    let deadline = monotonic_ns().saturating_add(100_000_000);
    while !super::ioapic::pending(gsi) && monotonic_ns() < deadline {
        core::hint::spin_loop();
    }
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
        io::write64(base + 0x100, u64::from(gsi) << 9);
    }
    let delivered = super::ioapic::pending(gsi);
    super::ioapic::acknowledge(gsi);
    super::ioapic::mask(gsi, true);
    assert!(delivered, "HPET interrupt did not traverse IOAPIC/IDT");
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    crate::log_line("kernel: Q35 HPET IOAPIC interrupt verified");
}
