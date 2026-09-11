use super::*;
use crate::arch::ArchAPI;

pub fn wait_for_interrupt() {
    super::interrupts::secure::progress();
    if super::interrupts::secure::pending() {
        return;
    }
    unsafe {
        asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

pub fn install_exception_vectors() {
    unsafe {
        let vectors = core::ptr::addr_of!(__exception_vectors) as u64;
        asm!("msr vbar_el1, {vectors:x}", vectors = in(reg) vectors, options(nostack));
        asm!("isb", options(nostack));
    }
}

pub fn jump_to_replacement(entry: u64, handoff: u64) -> ! {
    unsafe {
        asm!(
            "msr daifset, #0xf",
            "br x1",
            in("x0") handoff,
            in("x1") entry,
            options(noreturn)
        );
    }
}

pub fn enable_interrupts() {
    unsafe {
        asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
    }
}

pub fn exception_syndrome() -> u64 {
    let esr: u64;
    unsafe {
        asm!("mrs {esr:x}, esr_el1", esr = out(reg) esr, options(nomem, nostack));
    }
    esr
}

pub fn exception_link() -> u64 {
    let elr: u64;
    unsafe {
        asm!("mrs {elr:x}, elr_el1", elr = out(reg) elr, options(nomem, nostack));
    }
    elr
}

pub fn switch_address_space(address_space: AddressSpaceSwitch) {
    unsafe {
        hardening::speculation_barrier(hardening::SpeculationBarrier::DsbIsb);
        hardening::load_userspace_pac_key_for_switch(address_space.userspace_pac_key);
        let ttbr0 = address_space.root_table_phys | ((address_space.asid as u64) << 48);
        asm!("msr ttbr0_el1, {ttbr0:x}", ttbr0 = in(reg) ttbr0, options(nostack));
        if address_space.asid == 0 {
            asm!("dsb ishst; tlbi vmalle1; dsb ish; isb", options(nostack));
        } else {
            asm!("dsb ishst; isb", options(nostack));
        }
    }
}

pub fn enter_userspace(entry: u64, stack_top: u64) -> ! {
    unsafe {
        asm!(
            "msr sp_el0, {stack_top:x}",
            "msr elr_el1, {entry:x}",
            "msr spsr_el1, {spsr:x}",
            "eret",
            stack_top = in(reg) stack_top,
            entry = in(reg) entry,
            spsr = in(reg) 0_u64,
            options(noreturn)
        );
    }
}

pub fn handle_sync_exception(esr: u64, elr: u64, frame: *mut bexos_kernel_core::runtime::Context) {
    crate::userspace::RUNTIME.with(|s| {
        if let Some(rt) = s.as_mut() {
            let _ = rt.bind_current_cpu(crate::arch::CurrentArch::current_cpu_id() as u8);
        }
    });
    if (esr >> 26) & 63 == 0x15 {
        crate::syscall::dispatch(frame, (esr & 0xffff) as u32);
    } else if is_lower_el_write_fault(esr)
        && crate::userspace::RUNTIME.with(|s| {
            s.as_mut()
                .unwrap()
                .commit_user_write_fault(crate::arch::CurrentArch::fault_address())
                .is_ok()
        })
    {
        return;
    } else {
        bexos_trace::trace_counter!(
            bexos_trace::CATEGORY_KERNEL_SCHED,
            "kernel:sync_exception_esr",
            esr as i64
        );
        use core::fmt::Write;
        crate::state::UART.with(|s| {
            if let Some(u) = s.as_mut() {
                let far = crate::arch::CurrentArch::fault_address();
                let _ = writeln!(u, "guest fault esr={esr:x} pc={elr:x} far={far:x}");
            }
        });
        crate::userspace::RUNTIME.with(|s| s.as_mut().unwrap().exit_current());
        crate::sched::timer_tick(frame);
    }
}

fn is_lower_el_write_fault(esr: u64) -> bool {
    let ec = (esr >> 26) & 0x3f;
    let dfsc = esr & 0x3f;
    let write = ((esr >> 6) & 1) != 0;
    ec == 0x24 && write && matches!(dfsc, 0b001101 | 0b001111)
}
