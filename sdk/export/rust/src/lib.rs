#![no_std]

mod arch;
mod startup;

pub use arch::{exit, log, yield_now};
pub use startup::service_ready;

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .p2align 3
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .p2align 3
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .p2align 3
"#
);

#[macro_export]
macro_rules! entry {
    ($entry:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn _start(startup_channel: u64) -> ! {
            $entry(startup_channel)
        }

        #[panic_handler]
        fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
            $crate::log("out-of-tree app panic\n");
            $crate::exit()
        }
    };
}
