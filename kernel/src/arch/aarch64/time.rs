use super::*;

pub fn set_timer_interval(ticks: u64) {
    unsafe {
        asm!("msr cntv_tval_el0, {ticks:x}", ticks = in(reg) ticks, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {ctl:x}", ctl = in(reg) 1_u64, options(nomem, nostack));
    }
}

pub fn set_timer_deadline_ns(deadline_ns: u64) {
    let frequency = timer_frequency();
    let deadline_ticks = bexos_kernel_core::time::nanos_to_ticks(deadline_ns, frequency);
    unsafe {
        asm!("msr cntv_cval_el0, {deadline_ticks:x}", deadline_ticks = in(reg) deadline_ticks, options(nomem, nostack));
        asm!("msr cntv_ctl_el0, {ctl:x}", ctl = in(reg) 1_u64, options(nomem, nostack));
    }
}

pub fn disable_timer() {
    unsafe {
        asm!("msr cntv_ctl_el0, {ctl:x}", ctl = in(reg) 0_u64, options(nomem, nostack));
    }
}

pub fn timer_frequency() -> u64 {
    let frequency: u64;
    unsafe {
        asm!("mrs {frequency:x}, cntfrq_el0", frequency = out(reg) frequency, options(nomem, nostack));
    }
    frequency
}

pub fn timer_ticks() -> u64 {
    let ticks: u64;
    unsafe {
        asm!("mrs {ticks:x}, cntvct_el0", ticks = out(reg) ticks, options(nomem, nostack));
    }
    ticks
}

pub fn counter_value() -> u64 {
    let ticks: u64;
    unsafe {
        asm!("mrs {ticks:x}, cntpct_el0", ticks = out(reg) ticks, options(nomem, nostack));
    }
    ticks
}

pub fn monotonic_ns() -> u64 {
    bexos_kernel_core::time::ticks_to_nanos(counter_value(), timer_frequency())
}
