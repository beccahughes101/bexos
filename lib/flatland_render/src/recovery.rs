//! Device recovery bounds are distinct from the display frame deadline.
pub const HARDWARE_TIMEOUT_US: u64 = 2_000_000;

/// CPU Vulkan adapters can be used for functional validation under emulation.
/// Their explicit configuration must never relax a hardware adapter's bound.
pub fn timeout_us(software: bool, software_timeout_ms: u32) -> Option<u64> {
    if !(2_000..=30_000).contains(&software_timeout_ms) {
        return None;
    }
    Some(if software {
        u64::from(software_timeout_ms) * 1_000
    } else {
        HARDWARE_TIMEOUT_US
    })
}
