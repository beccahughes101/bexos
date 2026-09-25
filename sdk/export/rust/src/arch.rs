#[inline]
pub fn log(message: &str) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "svc #3",
            in("x0") message.as_ptr() as u64,
            in("x1") message.len() as u64,
            options(nostack)
        );
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") 3u64,
            in("rdi") message.as_ptr() as u64,
            in("rsi") message.len() as u64,
            options(nostack)
        );
    }
}

#[inline]
pub fn yield_now() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("svc #2", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") 2u64,
            in("rdi") 0u64,
            in("rsi") 0u64,
            options(nostack)
        );
    }
}

pub fn exit() -> ! {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("svc #4", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") 4u64,
            in("rdi") 0u64,
            in("rsi") 0u64,
            options(nostack)
        );
    }
    loop {
        yield_now();
    }
}

pub(crate) fn fidl(
    protocol: u64,
    ordinal: u64,
    request: &[u8],
    handles: &[u64],
    response: &mut [u8],
) -> Result<usize, i32> {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut status = protocol;
        let mut response_len = ordinal;
        let mut out_handles_len = request.as_ptr() as u64;
        core::arch::asm!(
            "svc #1",
            inout("x0") status,
            inout("x1") response_len,
            inout("x2") out_handles_len,
            in("x3") request.len() as u64,
            in("x4") handles.as_ptr() as u64,
            in("x5") handles.len() as u64,
            in("x6") response.as_mut_ptr() as u64,
            in("x7") response.len() as u64,
            in("x8") core::ptr::null_mut::<u64>() as u64,
            in("x9") 0u64,
            options(nostack)
        );
        if status as i32 == 0 && out_handles_len == 0 {
            Ok(response_len as usize)
        } else {
            Err(status as i32)
        }
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut status = protocol;
        let mut response_len = ordinal;
        let mut out_handles_len = request.as_ptr() as u64;
        core::arch::asm!(
            "int 0x80",
            in("rax") 1u64,
            inout("rdi") status,
            inout("rsi") response_len,
            inout("rdx") out_handles_len,
            in("r10") request.len(),
            in("r8") handles.as_ptr(),
            in("r9") handles.len(),
            in("r12") response.as_mut_ptr(),
            in("r13") response.len(),
            in("r14") core::ptr::null_mut::<u64>(),
            in("r15") 0usize,
            options(nostack)
        );
        if status as i32 == 0 && out_handles_len == 0 {
            Ok(response_len as usize)
        } else {
            Err(status as i32)
        }
    }
}
