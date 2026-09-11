//! Guest-visible CPU feature policy for the legacy x87/SSE context backend.
//! Filtering is independent of host CPUID execution so hostile/unsupported
//! feature combinations can be tested on either build architecture.
pub fn filter(leaf: u32, cpu: u8, count: u8, mut words: [u32; 4]) -> [u32; 4] {
    match leaf {
        1 => {
            words[1] = (words[1] & 0xffff) | (u32::from(count) << 16) | (u32::from(cpu) << 24);
            words[2] &= !((1 << 3)
                | (1 << 5)
                | (1 << 6)
                | (1 << 12)
                | (1 << 17)
                | (1 << 21)
                | (1 << 24)
                | (1 << 26)
                | (1 << 27)
                | (1 << 28)
                | (1 << 29));
            words[2] |= 1 << 31;
            if count > 1 {
                words[3] |= 1 << 28;
            } else {
                words[3] &= !(1 << 28);
            }
        }
        7 | 0xd | 0x8000000a => words = [0; 4],
        0xb | 0x1f => words = [0, 0, 0, u32::from(cpu)], // Legacy topology is in leaf 1.
        0x80000001 => {
            words[2] &= !((1 << 2) | (1 << 11) | (1 << 16) | (1 << 21));
            words[3] &= !(1 << 27); // RDTSCP's TSC_AUX is not switched.
        }
        0x80000008 => {
            words[0] = (words[0] & 0xffff0000) | (48 << 8) | 48;
            words[1] = 0;
            words[2] = u32::from(count.saturating_sub(1));
        }
        0x8000001e => words = [u32::from(cpu), u32::from(cpu), 0, 0],
        _ => {}
    }
    words
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_virtual_cpu_ids_do_not_leak_host_topology_or_unsaved_features() {
        for cpu in 0..4 {
            let result = filter(1, cpu, 4, [u32::MAX; 4]);
            assert_eq!(result[1] >> 24, u32::from(cpu));
            assert_eq!((result[1] >> 16) & 255, 4);
            assert_eq!(
                result[2] & ((1 << 26) | (1 << 27) | (1 << 28) | (1 << 21)),
                0
            );
            assert_ne!(result[2] & (1 << 31), 0);
            assert_eq!(filter(7, cpu, 4, [u32::MAX; 4]), [0; 4]);
            assert_eq!(filter(0x80000001, cpu, 4, [u32::MAX; 4])[3] & (1 << 27), 0);
        }
    }
}
