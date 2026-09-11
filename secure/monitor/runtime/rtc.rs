//! Sample the physical RTC before guest entry and before arming the NMI
//! watchdog. Guest accesses subsequently use only their private register image.
use core::arch::asm;
unsafe fn read(register: u8) -> u8 {
    let value: u8;
    unsafe {
        asm!("out dx, al", in("dx") 0x70u16, in("al") register, options(nomem, nostack));
        asm!("in al, dx", in("dx") 0x71u16, out("al") value, options(nomem, nostack));
    }
    value
}
pub unsafe fn snapshot() -> bexos_secure_monitor::cmos::Clock {
    let start = unsafe { bexos_secure_monitor::clock::now_ns() };
    loop {
        assert!(
            unsafe { bexos_secure_monitor::clock::now_ns() }.saturating_sub(start) < 1_000_000_000
        );
        if unsafe { read(0xa) } & 128 != 0 {
            continue;
        }
        let mut registers = [0; 128];
        for register in [0, 2, 4, 7, 8, 9, 0xa, 0xb, 0xd, 0x32] {
            registers[register] = unsafe { read(register as u8) };
        }
        if unsafe { read(0xa) } & 128 != 0 || unsafe { read(0) } != registers[0] {
            continue;
        }
        if let Some(clock) = bexos_secure_monitor::cmos::Clock::new(registers, unsafe {
            bexos_secure_monitor::clock::now_ns()
        }) {
            return clock;
        }
    }
}
