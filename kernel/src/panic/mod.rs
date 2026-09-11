use crate::arch::ArchAPI;
use core::fmt::Write;
use core::panic::PanicInfo;

use crate::state::UART;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(bexos_update_kernel)]
    if crate::transplant::prepare::in_callback() {
        crate::arch::CurrentArch::debug_break();
    }
    UART.with(|slot| {
        if let Some(uart) = slot.as_mut() {
            let _ = writeln!(uart, "kernel: panic: {info}");
        }
    });

    loop {
        crate::arch::CurrentArch::wait_for_interrupt();
    }
}
