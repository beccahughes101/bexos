#![no_std]
#![no_main]

static mut SEQUENCE: u64 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        let mut message = *b"starnix loop 0000000000000000\n";
        let sequence = unsafe { core::ptr::read_volatile(&raw const SEQUENCE) };
        for index in 0..16 {
            let nibble = ((sequence >> ((15 - index) * 4)) & 0xf) as u8;
            message[13 + index] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
        }
        unsafe { linux_write(1, message.as_ptr(), message.len()) };
        unsafe { core::ptr::write_volatile(&raw mut SEQUENCE, sequence.wrapping_add(1)) };
        for _ in 0..50_000_000 {
            core::hint::spin_loop();
        }
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn linux_write(fd: usize, bytes: *const u8, len: usize) -> isize {
    let result: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 1usize => result,
            in("rdi") fd,
            in("rsi") bytes,
            in("rdx") len,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

#[cfg(target_arch = "aarch64")]
unsafe fn linux_write(fd: usize, bytes: *const u8, len: usize) -> isize {
    let mut result = fd as isize;
    unsafe {
        core::arch::asm!(
            "svc #0",
            inlateout("x0") result,
            in("x1") bytes,
            in("x2") len,
            in("x8") 64usize,
            options(nostack),
        );
    }
    result
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
