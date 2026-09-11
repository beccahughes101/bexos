//! QEMU TF-A/Trusty boot transport. The registered buffer lives in BL33's
//! reserved pages and is released before normal-world kernel entry.
use bexos_trusty_boot::{
    approval,
    avb::Avb,
    ql::{self, Error, Transport},
};
use core::arch::asm;

const NOP: u64 = 0x3c00_0003;
const RESTART_LAST: u64 = 0x3c00_0000;
const RESTART_FIQ: u64 = 0x3c00_0002;
#[repr(align(4096))]
struct Pages([u8; 8192]);
static mut SHARED: Pages = Pages([0; 8192]);

pub fn approve(generation: u64, location: u32) -> Result<(), &'static str> {
    // BL33 has no normal-world interrupt handlers. Mask only Group 1 at the
    // non-secure GICv2 CPU interface, preserving Trusty's Group 0 timer/IPIs.
    // DAIF alone cannot prevent a pending normal IRQ from interrupting Trusty.
    let interrupts = BootInterrupts::mask();
    if call(0xbc00_000b, 2, 0, 0) != 2 {
        return Err("Trusty API negotiation failed");
    }
    bootstrap().map_err(|_| "Trusty bootstrap unavailable; refusing execution")?;
    let uart = super::rpmb::Uart::new().map_err(|_| "boot RPMB transport unavailable")?;
    let mut avb = Avb::connect_with_storage(Smc { uart })
        .map_err(|_| "Trusty rollback state unavailable; refusing execution")?;
    #[cfg(feature = "rollback_fixture")]
    {
        if generation == 0 || location > 31 || avb.read_lock_state() != Ok(true) {
            return Err("invalid rollback provisioning policy");
        }
        let floor = avb
            .read_rollback(location)
            .map_err(|_| "rollback fixture floor read failed")?;
        let next = generation
            .max(floor)
            .checked_add(1)
            .ok_or("rollback fixture floor overflow")?;
        avb.write_rollback(location, next)
            .map_err(|_| "rollback fixture floor write failed")?;
        if avb.read_rollback(location) != Ok(next) {
            return Err("rollback fixture floor readback failed");
        }
        avb.lock_boot_state()
            .map_err(|_| "rollback fixture boot seal failed")?;
        avb.close()
            .map_err(|_| "rollback fixture QL release failed")?;
        super::rpmb::release().map_err(|_| "rollback fixture RPMB release failed")?;
        super::log("bl33-fixture: authenticated RPMB floor provisioned\n");
        // A separately signed negative-test loader deliberately provisions a
        // newer floor. It can never transfer execution to the supplied kernel.
        loop {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }
    }
    // The TF-A-authenticated current Trusty image is generation 1. Its floor
    // is separate from the BexOS payload and cannot be approved by kernel AVB.
    let result = approval::approve_chain(
        &mut avb,
        [
            approval::Generation {
                generation,
                location,
            },
            approval::Generation {
                generation: 1,
                location: 30,
            },
        ],
    )
    .map_err(|error| match error {
        approval::Error::Rollback => "stale generation rejected by Trusty RPMB floor",
        approval::Error::Unlocked => "unlocked Trusty boot state rejected",
        approval::Error::InvalidGeneration => "invalid boot generation rejected",
        approval::Error::Transport(_) => "Trusty rollback state unavailable; refusing execution",
    })?[0];
    if avb.write_rollback(result.location(), result.floor()) != Err(Error::Rejected) {
        return Err("Trusty boot mutation seal failed");
    }
    avb.close().map_err(|_| "Trusty boot QL release failed")?;
    // Only after all synchronous frames and QL handles are closed may the
    // helper accept the queued normal-world connection to the same device.
    super::rpmb::release().map_err(|_| "boot RPMB owner release failed")?;
    drop(interrupts);
    super::log("bl33: authenticated payload and Trusty rollback approval verified\n");
    Ok(())
}

struct BootInterrupts(u32);
impl BootInterrupts {
    fn mask() -> Self {
        unsafe {
            let control = core::ptr::read_volatile(0x0801_0000 as *const u32);
            core::ptr::write_volatile(0x0801_0000 as *mut u32, control & !1);
            asm!("dsb sy", "isb", options(nostack));
            Self(control)
        }
    }
}
impl Drop for BootInterrupts {
    fn drop(&mut self) {
        unsafe {
            core::ptr::write_volatile(0x0801_0000 as *mut u32, self.0);
            asm!("dsb sy", "isb", options(nostack));
        }
    }
}
fn bootstrap() -> Result<(), Error> {
    let started = now_ns();
    let mut operation = NOP;
    loop {
        let now = now_ns();
        if now < started || now - started >= 30_000_000_000 {
            return Err(Error::Timeout);
        }
        match call(operation, 0, 0, 0) {
            0 | -15 => return Ok(()),
            -14 | -5 | -13 => operation = NOP,
            -12 => operation = RESTART_FIQ,
            -3 => operation = RESTART_LAST,
            _ => return Err(Error::Rejected),
        }
    }
}
struct Smc {
    uart: super::rpmb::Uart,
}
impl Transport for Smc {
    fn exchange(&mut self, operation: u32, length: usize, buffer: &mut [u8]) -> Result<i64, Error> {
        if !matches!(operation, ql::CREATE | ql::COMMAND | ql::SHUTDOWN)
            || buffer.len() > 8192
            || length > buffer.len()
        {
            return Err(Error::Transport);
        }
        let shared = unsafe { &mut *core::ptr::addr_of_mut!(SHARED.0) };
        shared.fill(0);
        shared[..buffer.len()].copy_from_slice(buffer);
        let pa = shared.as_ptr() as u64;
        let id = (pa & 0xffff_ffff_f000) | (0xff << 48) | (3 << 8);
        unsafe {
            asm!("dsb sy", options(nostack));
        }
        let result = restart([
            u64::from(operation),
            id as u32 as u64,
            id >> 32,
            length as u64,
        ])?;
        unsafe {
            asm!("dsb sy", options(nostack));
        }
        buffer.copy_from_slice(&shared[..buffer.len()]);
        if operation == ql::SHUTDOWN && result == 0 {
            shared.fill(0);
        }
        Ok(result)
    }
    fn now_ns(&self) -> u64 {
        now_ns()
    }
    fn storage(
        &mut self,
        request: &[u8],
        response: &mut [u8; bexos_trusty_boot::storage::MAX_RESPONSE],
    ) -> Result<usize, Error> {
        bexos_trusty_boot::storage::dispatch(&mut self.uart, request, response)
    }
}
pub fn now_ns() -> u64 {
    let (ticks, frequency): (u64, u64);
    unsafe {
        asm!("isb", "mrs {ticks}, cntpct_el0", "mrs {frequency}, cntfrq_el0", ticks=out(reg) ticks, frequency=out(reg) frequency, options(nomem, nostack));
    }
    if frequency == 0 {
        super::fail("invalid boot timer");
    }
    ((u128::from(ticks) * 1_000_000_000) / u128::from(frequency)).min(u128::from(u64::MAX)) as u64
}
fn restart(original: [u64; 4]) -> Result<i64, Error> {
    let started = now_ns();
    let mut current = original;
    loop {
        let now = now_ns();
        if now < started || now - started >= 30_000_000_000 {
            return Err(Error::Timeout);
        }
        let result = call(current[0], current[1], current[2], current[3]);
        match result {
            -3 | -13 => current = [RESTART_LAST, 0, 0, 0],
            -12 => current = [RESTART_FIQ, 0, 0, 0],
            -14 => current = [NOP, 0, 0, 0],
            -5 => core::hint::spin_loop(),
            -15 if current[0] == NOP => current = original,
            _ => return Ok(result),
        }
    }
}
fn call(mut x0: u64, mut x1: u64, mut x2: u64, mut x3: u64) -> i64 {
    unsafe {
        // Match the pinned bootloader SMC ABI, including all caller-saved
        // registers. x20 survives the secure call and holds the IRQ mask.
        asm!("mrs x20, daif", "msr daifset, #2", "smc #0", "msr daif, x20",
            inout("x0") x0, inout("x1") x1, inout("x2") x2, inout("x3") x3,
            out("x20") _, clobber_abi("C"), options(nostack));
    }
    let _ = (x1, x2, x3);
    x0 as i32 as i64
}
