#![no_std]
#![no_main]
use bexos_secure_monitor::policy_image as abi;
use core::arch::{asm, global_asm};

const TAG: u64 = if cfg!(feature = "successor") {
    3
} else if cfg!(feature = "candidate") {
    2
} else {
    1
};
#[used]
#[unsafe(link_section = ".monitor.abi")]
static DESCRIPTOR: [u64; 8] = abi::descriptor(TAG);
static mut CALLS: u64 = 0;

global_asm!(
    r#"
.section .text.entry,"ax"
.global _start
_start:
    jmp policy_turn
"#
);

/// No pointer, hardware ownership, or suspended guest operation crosses this
/// interface. A returned work decision is checked before any guest executes.
#[unsafe(no_mangle)]
extern "C" fn policy_turn(header: u64, sequence: u64, _now: u64, secure_pending: u64) -> u64 {
    if header != abi::CALL || secure_pending > 1 {
        return abi::FAILURE;
    }
    let calls = unsafe {
        let n = core::ptr::read_volatile(core::ptr::addr_of!(CALLS)).wrapping_add(1);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CALLS), n);
        n
    };
    if calls >= 4 {
        #[cfg(feature = "fault")]
        unsafe {
            asm!("ud2", options(noreturn));
        }
        #[cfg(feature = "hang")]
        unsafe {
            asm!("2: jmp 2b", options(noreturn));
        }
    }
    let normal = if secure_pending != 0 {
        1
    } else if TAG == 1 {
        32
    } else {
        // Keep cold-boot service throughput close to the baseline while the
        // distinct image varies bounded normal work at each resident boundary.
        24 + (sequence & 7)
    };
    (abi::RESPONSE << 32) | (TAG << 24) | (1 << 8) | normal
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe {
        asm!("ud2", options(noreturn));
    }
}
