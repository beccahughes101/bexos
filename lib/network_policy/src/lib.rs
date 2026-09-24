#![no_std]

/// Converts a configured physical-interface selector into a stable, nonzero
/// identifier shared by app-facing topology and vswitch device discovery.
/// Decimal selectors retain their explicit numeric identity.
pub fn selector_id(selector: &str) -> Option<u64> {
    if selector.is_empty() || selector.len() > 128 || !selector.is_ascii() {
        return None;
    }
    if let Ok(value) = selector.parse::<u64>() {
        return (value != 0).then_some(value);
    }
    // Product policy uses this explicit selector for the sole QEMU NIC;
    // vswitchd resolves zero to the first enumerated physical interface.
    if selector == "qemu-default" {
        return Some(0);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in b"bexos-network-selector:".iter().chain(selector.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Some(hash.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_stable_and_numeric_ids_are_preserved() {
        assert_eq!(selector_id("17"), Some(17));
        assert_eq!(selector_id("qemu-default"), Some(0));
        assert_ne!(selector_id("qemu-default"), selector_id("corp-uplink"));
        assert_eq!(selector_id(""), None);
        assert_eq!(selector_id("0"), None);
    }
}
