//! Amortize world switches across quick normal-world emulated device exits.
//! Trusty receives service at least once per bounded burst, and a submitted or
//! running secure request immediately ends the normal burst. NMI interception
//! still limits a single guest entry even when the guest masks interrupts.
use crate::normal::Normal;

const NORMAL_BURST_NS: u64 = 1_000_000;
const MAX_NORMAL_EXITS: usize = 32;

pub unsafe fn normal_turn(normal: &mut Normal) {
    let started = unsafe { bexos_secure_monitor::clock::now_ns() };
    for _ in 0..MAX_NORMAL_EXITS {
        unsafe {
            normal.step();
            #[cfg(feature = "normal_checkpoint_probe")]
            normal.checkpoint_probe();
            let now = bexos_secure_monitor::clock::now_ns();
            if crate::transport::needs_secure_progress()
                || now < started
                || now - started >= NORMAL_BURST_NS
            {
                break;
            }
        }
    }
}
