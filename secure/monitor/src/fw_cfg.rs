//! Bounded QEMU firmware-file reads before domain entry. The fw_cfg selector
//! and data ports remain intercepted and unavailable to guest domains.
use core::arch::asm;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileError {
    NotFound,
    Invalid,
}
unsafe fn select(item: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") 0x510u16, in("ax") item, options(nomem, nostack));
    }
}
unsafe fn byte() -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") 0x511u16, out("al") value, options(nomem, nostack));
    }
    value
}
unsafe fn be_number(count: usize) -> u32 {
    let mut value = 0;
    for _ in 0..count {
        value = (value << 8) | u32::from(unsafe { byte() });
    }
    value
}
/// # Safety
/// The monitor exclusively owns fw_cfg port access. The host provides immutable
/// files; callers must validate the required size and purpose of their data.
pub unsafe fn read(name: &[u8], output: &mut [u8]) -> bool {
    if output.is_empty() || output.len() > 4096 {
        return false;
    }
    unsafe {
        let Ok((selector, length)) = lookup(name, output.len()) else {
            return false;
        };
        if length != output.len() {
            return false;
        }
        select(selector);
        for value in output {
            *value = byte();
        }
        true
    }
}

/// # Safety
/// Same exclusive port ownership as `read`. A missing optional file is distinct
/// from a malformed directory or oversized file; callers must reject the latter.
pub unsafe fn read_bounded(name: &[u8], output: &mut [u8]) -> Result<usize, FileError> {
    if output.is_empty() || output.len() > 4096 {
        return Err(FileError::Invalid);
    }
    unsafe {
        let (selector, length) = lookup(name, output.len())?;
        select(selector);
        for value in &mut output[..length] {
            *value = byte();
        }
        Ok(length)
    }
}

unsafe fn lookup(name: &[u8], capacity: usize) -> Result<(u16, usize), FileError> {
    if name.is_empty() || name.len() >= 56 || name.contains(&0) {
        return Err(FileError::Invalid);
    }
    unsafe {
        select(0x19);
        let count = be_number(4);
        if count > 256 {
            return Err(FileError::Invalid);
        }
        let mut selected = None;
        for _ in 0..count {
            let size = be_number(4);
            let selector = be_number(2) as u16;
            let reserved = be_number(2);
            let mut entry = [0; 56];
            for value in &mut entry {
                *value = byte();
            }
            if entry[..name.len()] == *name && entry[name.len()] == 0 {
                if selected.is_some()
                    || reserved != 0
                    || size == 0
                    || size as usize > capacity
                    || !(0x20..0x4000).contains(&selector)
                {
                    return Err(FileError::Invalid);
                }
                selected = Some((selector, size as usize));
            }
        }
        selected.ok_or(FileError::NotFound)
    }
}

/// Read a complete bounded firmware file into private identity-mapped memory.
/// No device address or length comes from a guest domain. Authentication of
/// these untrusted bytes must complete before they become executable.
///
/// # Safety
/// Called once during root bootstrap, before domain entry and DMA assignment.
/// `output` and this function's stack must be private identity-mapped RAM.
/// QEMU's fw_cfg DMA implementation completes synchronously. A nonzero status
/// fails closed; the caller must halt rather than allow a pending DMA to outlive
/// the bootstrap storage.
pub unsafe fn read_dma(name: &[u8], output: &mut [u8]) -> Option<usize> {
    if output.is_empty() || output.len() > crate::boot_verify::BOOTFS_MAX_BYTES {
        return None;
    }
    unsafe {
        select(1);
        let features = u32::from_le_bytes([byte(), byte(), byte(), byte()]);
        if features & 2 == 0 {
            return None;
        }
        let (selector, length) = lookup(name, output.len()).ok()?;
        #[repr(C, align(16))]
        struct Access {
            control: u32,
            length: u32,
            address: u64,
        }
        let mut access = Access {
            control: ((u32::from(selector) << 16) | 0xa).to_be(),
            length: (length as u32).to_be(),
            address: (output.as_mut_ptr() as u64).to_be(),
        };
        let address = core::ptr::addr_of_mut!(access) as u64;
        // QEMU specifies a big-endian address, high half first. Writing the
        // low half triggers the transfer. Deliberately retain memory clobbers.
        asm!("out dx, eax", in("dx") 0x514u16, in("eax") ((address >> 32) as u32).to_be(), options(nostack));
        asm!("out dx, eax", in("dx") 0x518u16, in("eax") (address as u32).to_be(), options(nostack));
        (core::ptr::read_volatile(core::ptr::addr_of!(access.control)) == 0).then_some(length)
    }
}
