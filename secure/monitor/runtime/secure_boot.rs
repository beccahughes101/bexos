//! Cold restart of the isolated secure domain during firmware recovery.
//! No normal domain may have been admitted and no transport owner may be live.
use bexos_secure_monitor::trial_rpmb::{Error, Uart};
use bexos_secure_monitor::{image::Image, svm::Vmcb, vcpu::Registers};
use core::sync::atomic::{AtomicBool, Ordering};

#[unsafe(link_section = ".resident.recovery")]
static FENCED: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".resident.recovery")]
static mut FENCED_BANK: u64 = 0;
#[unsafe(link_section = ".resident.trial_uart")]
static mut TRIAL_UART: Uart = Uart::new();

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub fn begin_live_trial(bank: u64) {
    unsafe {
        TRIAL_UART = Uart::new();
        FENCED_BANK = bank;
    }
    FENCED.store(true, Ordering::Release);
}

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub fn finish_live_trial() {
    FENCED.store(false, Ordering::Release);
    unsafe {
        FENCED_BANK = 0;
    }
}

/// Parse an entire request before forwarding read-only RPMB traffic. A trial
/// can demonstrate real storage health without programming or changing RPMB.
pub fn fenced_rpmb_port(bank: u64, port: u16, input: bool, value: u8) -> Option<u8> {
    if FENCED.load(Ordering::Acquire)
        && bank == unsafe { FENCED_BANK }
        && (0x2f8..=0x2ff).contains(&port)
    {
        Some(
            unsafe {
                (&mut *core::ptr::addr_of_mut!(TRIAL_UART)).port(
                    port - 0x2f8,
                    (!input).then_some(value),
                    read_only_exchange,
                )
            }
            .unwrap_or_else(|_| {
                crate::log(
                    "monitor-runtime: firmware recovery required; trial RPMB transport failed\n",
                );
                crate::halt()
            }),
        )
    } else {
        None
    }
}

pub fn blocked_writes() -> u64 {
    unsafe { (&*core::ptr::addr_of!(TRIAL_UART)).blocked() }
}
pub fn completed_reads() -> u64 {
    unsafe { (&*core::ptr::addr_of!(TRIAL_UART)).reads() }
}
fn read_only_exchange(request: &[u8], response: &mut [u8]) -> Result<(), Error> {
    use core::arch::asm;
    let started = unsafe { bexos_secure_monitor::clock::now_ns() };
    let wait = |mask: u8| -> Result<(), Error> {
        loop {
            let status: u8;
            unsafe {
                asm!("in al, dx", in("dx") 0x2fdu16, out("al") status, options(nomem, nostack));
            }
            if status & mask != 0 {
                return Ok(());
            }
            if unsafe { bexos_secure_monitor::clock::now_ns() }.saturating_sub(started)
                >= 5_000_000_000
            {
                return Err(Error::Transport);
            }
            core::hint::spin_loop();
        }
    };
    for &byte in request {
        wait(0x20)?;
        unsafe {
            asm!("out dx, al", in("dx") 0x2f8u16, in("al") byte, options(nomem, nostack));
        }
    }
    for byte in response {
        wait(1)?;
        unsafe {
            asm!("in al, dx", in("dx") 0x2f8u16, out("al") *byte, options(nomem, nostack));
        }
    }
    Ok(())
}

pub unsafe fn restart(
    bytes: &[u8],
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut crate::platform::Platform<1>,
    fenced: bool,
) {
    let mut alignment = core::mem::MaybeUninit::<Vmcb>::uninit();
    core::hint::black_box(&mut alignment);
    let image = Image::parse(bytes, crate::BANK_SIZE).unwrap();
    FENCED.store(fenced, Ordering::Release);
    unsafe {
        TRIAL_UART = Uart::new();
        crate::transport::reset_recovery();
        let memory = core::slice::from_raw_parts_mut(crate::BANK as *mut u8, crate::BANK_SIZE);
        memory.fill(0);
        image.load(memory).unwrap();
        for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007)] {
            memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        for page in 0..512 {
            memory[0x3000 + page * 8..0x3008 + page * 8]
                .copy_from_slice(&(page as u64 * 0x200000 | 0x87).to_le_bytes());
        }
        *vmcb = Vmcb::new();
        vmcb.initialize(
            1,
            (&*core::ptr::addr_of!(crate::TABLES)).root().unwrap(),
            0x1000,
            image.entry(),
            0x10000,
        )
        .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(crate::IOPM) as u64,
            core::ptr::addr_of!(crate::MSRPM) as u64,
        )
        .unwrap();
    }
    vmcb.intercept_cpuid();
    vmcb.intercept_nmi();
    *regs = Registers::default();
    *platform = crate::platform::Platform::new(
        crate::memory::DomainMemory {
            base: crate::BANK,
            length: crate::BANK_SIZE,
        },
        true,
    );
}
