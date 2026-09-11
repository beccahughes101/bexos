#![no_std]
#![no_main]
use bexos_userspace::{Channel, Memory, Startup, log, syscall, yield_now};
bexos_userspace::entry!(run);
fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).unwrap();
    let vmo = Memory::create(4096, 0).unwrap();
    let address = Memory::map(vmo, 4096, if startup.arg0 == 1 { 2 } else { 6 }).unwrap();
    Startup::ready(control).unwrap();
    // Allow appd to acknowledge readiness before deliberately faulting.
    let until = syscall::ticks().saturating_add(syscall::frequency() / 2);
    while syscall::ticks() < until {
        yield_now();
    }
    log("fault-probe: executing prohibited access\n");
    let address = if startup.arg0 == 3 { 0 } else { address };
    unsafe {
        if startup.arg0 == 2 {
            let execute: unsafe extern "C" fn() = core::mem::transmute(address as usize);
            execute();
        } else {
            #[cfg(all(bexos_guest, target_arch = "x86_64"))]
            core::arch::asm!("mov qword ptr [{address}], 1", address = in(reg) address, options(nostack));
            #[cfg(all(bexos_guest, target_arch = "aarch64"))]
            core::arch::asm!("str xzr, [{address}]", address = in(reg) address, options(nostack));
        }
    }
    log("fault-probe: ERROR prohibited access returned\n");
    bexos_userspace::exit()
}
