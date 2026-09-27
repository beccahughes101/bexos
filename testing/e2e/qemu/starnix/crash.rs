#![no_std]
#![no_main]

const MESSAGE: &[u8] = b"starnix crash fixture: injecting fault\n";

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe { linux_write(1, MESSAGE.as_ptr(), MESSAGE.len()) };
    unsafe { fault() }
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

#[cfg(target_arch = "x86_64")]
unsafe fn fault() -> ! {
    unsafe { core::arch::asm!("ud2", options(noreturn)) }
}

#[cfg(target_arch = "aarch64")]
unsafe fn fault() -> ! {
    unsafe { core::arch::asm!("brk #0", options(noreturn)) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
