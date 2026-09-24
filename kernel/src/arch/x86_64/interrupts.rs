use super::{context, cpu, io, time};
use crate::arch::ArchAPI;
use core::sync::atomic::{AtomicU64, Ordering};
static BASE: AtomicU64 = AtomicU64::new(0xfee00000);
pub fn read(offset: u64) -> u32 {
    unsafe { io::read32(BASE.load(Ordering::Acquire) + offset) }
}
pub fn write(offset: u64, value: u32) {
    unsafe {
        io::write32(BASE.load(Ordering::Acquire) + offset, value);
    }
}
pub fn discover(lapic: u64, ioapic: u64) {
    BASE.store(lapic, Ordering::Release);
    unsafe {
        io::wrmsr(0x1b, io::rdmsr(0x1b) | (1 << 11));
    }
    super::ioapic::initialize(ioapic, true);
    initialize_local();
}
fn initialize_local() {
    write(0xf0, 0x100 | 255);
    write(0x80, 0);
    for reg in [0x320, 0x330, 0x340, 0x350, 0x360, 0x370] {
        write(reg, 1 << 16);
    }
}
pub fn quiesce_firmware_irqs() {
    unsafe {
        io::out8(0x21, 0xff);
        io::out8(0xa1, 0xff);
    }
}
pub fn init(_cpus: u32) {
    context::install_exception_vectors();
    initialize_local();
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    time::verify_ioapic_route();
    crate::log_line("kernel: APIC/tickless timer enabled");
}
pub fn init_secondary() {
    context::install_exception_vectors();
    initialize_local();
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}
pub fn program_scheduler_deadline(deadline: Option<u64>) {
    if let Some(deadline) = deadline {
        time::set_timer_deadline_ns(deadline);
    } else {
        time::disable_timer();
    }
}
pub fn send(apic: u8, command: u32) {
    while read(0x300) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
    write(0x310, (apic as u32) << 24);
    write(0x300, command);
    while read(0x300) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
}
pub fn request_reschedule(mask: u64) {
    for cpu_id in 0..64 {
        if mask & (1u64 << cpu_id) != 0 {
            send(cpu::apic_id(cpu_id), 33);
        }
    }
}
pub fn dispatch(frame: *mut bexos_kernel_core::runtime::Context, vector: u64) {
    if vector == 255 {
        return;
    }
    if vector == 32 {
        crate::sched::timer_tick(frame);
        crate::transplant::prepare::recover_timeout(frame);
    } else if vector == 33 {
        if cpu::current_cpu_id() != 0 && crate::transplant::cpu::parking_requested() {
            write(0xb0, 0);
            crate::transplant::cpu::park_current_cpu();
        }
        crate::sched::reschedule_ipi(frame);
    } else {
        if super::ioapic::dispatch(vector) {
            let gsi = (vector - 64) as u32;
            crate::userspace::RUNTIME.with(|state| {
                if let Some(runtime) = state.as_mut() {
                    let _ = runtime.deliver_interrupt(gsi, super::X86_64::monotonic_ns());
                }
            });
        }
    }
    write(0xb0, 0);
    if super::X86_64::is_userspace(unsafe { &*frame }) {
        crate::transplant::live_step();
    }
}

pub fn capture_base() -> u64 {
    BASE.load(Ordering::Acquire) | (super::ioapic::base() << 32)
}
pub fn restore_base(base: u64) {
    BASE.store(base & 0xffff_ffff, Ordering::Release);
    super::ioapic::initialize(base >> 32, false);
    initialize_local();
}
