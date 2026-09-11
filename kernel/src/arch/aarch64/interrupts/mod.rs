use core::fmt::Write;

use crate::arch::aarch64::{self, TrapFrame};
use crate::sched;
use crate::state::UART;

pub mod secure;

const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;
const TIMER_IRQ: u32 = 30;
const SCHEDULER_SGI: u32 = 1;

const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER: usize = 0x100;
const GICD_ICENABLER: usize = 0x180;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;
const GICD_SGIR: usize = 0xf00;

const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_IAR: usize = 0x00c;
const GICC_EOIR: usize = 0x010;

pub fn init(max_cpus: u32) {
    crate::log_line("kernel: gic init begin");
    write32(GICD_BASE + GICD_CTLR, 0);
    crate::log_line("kernel: gic distributor disabled");
    reset_normal_world_irqs();
    crate::log_line("kernel: inherited peripheral irqs masked");
    set_priority(TIMER_IRQ, 0x80);
    set_priority(SCHEDULER_SGI, 0x70);
    set_target_cpus(TIMER_IRQ, max_cpus);
    crate::log_line("kernel: gic priorities targeted");
    enable_irq(TIMER_IRQ);
    enable_irq(SCHEDULER_SGI);
    secure::enable_local();
    crate::log_line("kernel: gic irqs enabled");
    init_cpu_interface();
    crate::log_line("kernel: gic cpu interface enabled");
    init_local_timer();
    crate::log_line("kernel: local timer initialized");
    write32(GICD_BASE + GICD_CTLR, 1);
    aarch64::enable_interrupts();

    UART.with(|slot| {
        if let Some(uart) = slot.as_mut() {
            let _ = writeln!(
                uart,
                "kernel: gic/tickless timer enabled irq={TIMER_IRQ} sgi={SCHEDULER_SGI} max_cpus={max_cpus}"
            );
        }
    });
}

/// Quiesce non-secure interrupt sources inherited from firmware before any
/// PSCI or Trusty calls. Trusty's primary CPU can return to the normal world
/// before its IRQ handoff thread is ready when an old timer/PPI is pending;
/// entering another secure call in that state otherwise livelocks the secure
/// monitor on the same interrupt.
pub fn quiesce_firmware_irqs() {
    aarch64::disable_timer();
    reset_normal_world_irqs();
}

pub fn init_secondary() {
    init_cpu_interface();
    init_local_timer();
    secure::enable_local();
    secure::enqueue();
    aarch64::enable_interrupts();
}

fn init_cpu_interface() {
    write32(GICC_BASE + GICC_PMR, 0xff);
    write32(GICC_BASE + GICC_CTLR, 1);
}

fn init_local_timer() {
    // There is no periodic tick.  A runnable task, timeout, or quota
    // replenishment arms this CPU's generic timer with an absolute deadline.
    aarch64::disable_timer();
}

fn reset_normal_world_irqs() {
    // Firmware and emulated PCI devices can leave level-triggered SPIs pending
    // before their userspace drivers exist. Mask them in the non-secure GIC
    // view so unmasking DAIF cannot livelock the bootstrap IRQ handler.
    for register in 1..32 {
        write32(GICD_BASE + GICD_ICENABLER + register * 4, u32::MAX);
        write32(GICD_BASE + GICD_ICPENDR + register * 4, u32::MAX);
    }
    let unused_ppis = u32::MAX & !(1 << TIMER_IRQ);
    write32(GICD_BASE + GICD_ICENABLER, unused_ppis);
    // A transplant imports secure ownership before reinitializing the GIC.
    // Keep pending secure wakeups for the next provider call.
    write32(GICD_BASE + GICD_ICPENDR, unused_ppis & !secure::mask());
}

pub fn program_scheduler_deadline(deadline_ns: Option<u64>) {
    if let Some(deadline_ns) = deadline_ns {
        aarch64::set_timer_deadline_ns(deadline_ns);
    } else {
        aarch64::disable_timer();
    }
}

/// Send an SGI to CPUs that may have acquired runnable work while they were
/// idle.  CPU0 is included: the sender's local reschedule is harmless and
/// gives cross-CPU wakeups one uniform path.
pub fn request_reschedule(mask: u64) {
    let targets = (mask & 0xff) as u8;
    if targets != 0 {
        write32(
            GICD_BASE + GICD_SGIR,
            SCHEDULER_SGI | ((targets as u32) << 16),
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn handle_irq(frame: *mut TrapFrame) {
    let frame = frame.cast::<bexos_kernel_core::runtime::Context>();
    let iar = read32(GICC_BASE + GICC_IAR);
    let irq = iar & 0x3ff;
    if secure::owns(irq) {
        secure::mask_delivered(irq);
        write32(GICC_BASE + GICC_EOIR, iar);
        secure::enqueue();
        return;
    }
    if irq == SCHEDULER_SGI
        && aarch64::current_cpu_id() != 0
        && crate::transplant::cpu::parking_requested()
        && unsafe { TrapFrame::view(&*frame).spsr & 15 != 0 }
    {
        write32(GICC_BASE + GICC_EOIR, iar);
        crate::transplant::cpu::park_current_cpu();
    }
    bexos_trace::trace_counter!(bexos_trace::CATEGORY_KERNEL_SCHED, "kernel:irq", irq as i64,);

    if irq == TIMER_IRQ {
        sched::timer_tick(frame);
        crate::transplant::prepare::recover_timeout(frame);
    } else if irq == SCHEDULER_SGI {
        sched::reschedule_ipi(frame);
    }

    write32(GICC_BASE + GICC_EOIR, iar);
    if unsafe { TrapFrame::view(&*frame).spsr & 15 == 0 } {
        crate::transplant::live_step();
    }
}

fn enable_irq(irq: u32) {
    let register = GICD_BASE + GICD_ISENABLER + ((irq / 32) as usize * 4);
    write32(register, 1 << (irq % 32));
}

fn set_priority(irq: u32, priority: u8) {
    let register = GICD_BASE + GICD_IPRIORITYR + irq as usize;
    write8(register, priority);
}

fn set_target_cpus(irq: u32, max_cpus: u32) {
    let register = GICD_BASE + GICD_ITARGETSR + irq as usize;
    let targets = if max_cpus >= 8 {
        u8::MAX
    } else {
        ((1u16 << max_cpus) - 1) as u8
    };
    write8(register, targets);
}

fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn write32(addr: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(addr as *mut u32, value);
    }
}

fn write8(addr: usize, value: u8) {
    unsafe {
        core::ptr::write_volatile(addr as *mut u8, value);
    }
}
