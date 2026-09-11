//! Monitor-owned Q35 HPET clock. Guests cannot supply lifecycle timestamps.
use core::sync::atomic::{AtomicU64, Ordering};
const HPET: u64 = 0xfed00000;
#[unsafe(link_section = ".resident.clock")]
static PERIOD_FS: AtomicU64 = AtomicU64::new(0);
/// # Safety
/// The monitor owns and identity-maps Q35's HPET registers. Domain mappings
/// must not expose writable access to this clock or its configuration.
pub unsafe fn initialize() -> bool {
    unsafe {
        let capabilities = core::ptr::read_volatile(HPET as *const u64);
        let period = capabilities >> 32;
        if capabilities & (1 << 13) == 0 || period == 0 || period > 100_000_000 {
            return false;
        }
        let configuration = (HPET + 0x10) as *mut u64;
        core::ptr::write_volatile(configuration, core::ptr::read_volatile(configuration) | 1);
        PERIOD_FS.store(period, Ordering::Release);
        true
    }
}
/// # Safety
/// The private HPET mapping must remain valid after successful initialization.
pub unsafe fn now_ns() -> u64 {
    let period = PERIOD_FS.load(Ordering::Acquire);
    assert!(period != 0);
    let ticks = unsafe { core::ptr::read_volatile((HPET + 0xf0) as *const u64) };
    ((ticks as u128 * period as u128) / 1_000_000).min(u64::MAX as u128) as u64
}
/// Raw counter deadline for the resident NMI handler, which cannot call into
/// an image that may be faulting. The root HPET configuration is immutable.
pub unsafe fn deadline_ticks(duration_ns: u64) -> Option<u64> {
    let period = PERIOD_FS.load(Ordering::Acquire);
    if period == 0 || duration_ns == 0 {
        return None;
    }
    let delta = (u128::from(duration_ns) * 1_000_000).div_ceil(u128::from(period));
    let now = unsafe { core::ptr::read_volatile((HPET + 0xf0) as *const u64) };
    now.checked_add(u64::try_from(delta).ok()?)
}
