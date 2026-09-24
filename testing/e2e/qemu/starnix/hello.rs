#![no_std]
#![no_main]

const MESSAGE: &[u8] = b"hello starnix\n";

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    unsafe { linux_write(1, MESSAGE.as_ptr(), MESSAGE.len()) };
    unsafe { linux_exit_group(0) }
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
unsafe fn linux_exit_group(status: usize) -> ! {
    unsafe { core::arch::asm!("syscall", in("rax") 231usize, in("rdi") status, options(noreturn)) }
}

#[cfg(target_arch = "aarch64")]
unsafe fn linux_exit_group(status: usize) -> ! {
    unsafe { core::arch::asm!("svc #0", in("x0") status, in("x8") 94usize, options(noreturn)) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { linux_exit_group(127) }
}
