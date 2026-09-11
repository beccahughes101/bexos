use super::*;
#[test]
fn pending_priority_mask_and_eoi_are_per_cpu() {
    let mut a = LocalApic::new(0);
    let b = LocalApic::new(1);
    a.raise(32);
    assert_eq!(a.pending(), None);
    a.write(0xf0, 0x1ff, 0).unwrap();
    assert_eq!(a.pending(), Some(32));
    a.delivered(32);
    assert_eq!(a.pending(), None);
    a.raise(33);
    assert_eq!(a.pending(), None);
    a.raise(64);
    assert_eq!(a.pending(), Some(64));
    a.delivered(64);
    a.write(0xb0, 0, 0).unwrap();
    assert_eq!(a.pending(), None);
    a.write(0xb0, 0, 0).unwrap();
    assert_eq!(a.pending(), Some(33));
    a.write(0x80, 0x30, 0).unwrap();
    assert_eq!(a.pending(), None);
    assert_eq!(b.read(0x110, 0), Some(0));
    assert_eq!(b.read(0x20, 0), Some(1 << 24));
}
#[test]
fn timers_use_monitor_time_and_coalesce_missed_periods() {
    let mut apic = LocalApic::new(0);
    apic.write(0xf0, 0x1ff, 0).unwrap();
    apic.write(0x3e0, 3, 0).unwrap();
    apic.write(0x320, PERIODIC | 32, 0).unwrap();
    apic.write(0x380, 100, 1000).unwrap();
    assert_eq!(apic.read(0x390, 9000), Some(50));
    apic.tick(16999);
    assert_eq!(apic.pending(), None);
    apic.tick(17000);
    assert_eq!(apic.pending(), Some(32));
    apic.delivered(32);
    apic.write(0xb0, 0, 0).unwrap();
    apic.tick(1_000_000_000_000);
    assert_eq!(apic.pending(), Some(32));
    apic.write(0x380, 0, 0).unwrap();
    assert_eq!(apic.read(0x390, u64::MAX), Some(0));
}
#[test]
fn device_control_is_bounded_and_ipis_require_domain_routing() {
    let mut apic = LocalApic::new(0);
    assert!(apic.write(0x320, 4 << 8, 0).is_err()); // No physical NMI programming.
    assert!(apic.write(0x300, 0x840, 0).is_err()); // Unsupported logical destination.
    assert!(apic.write(0x3e0, u32::MAX, 0).is_err());
    apic.write(0x310, 3 << 24, 0).unwrap();
    assert_eq!(
        apic.write(0x300, 0x608, 0),
        Ok(Some(Ipi {
            destination: 3,
            shorthand: 0,
            kind: IpiKind::Startup(8)
        }))
    );
    assert!(!apic.raise(15));
}
#[test]
fn divider_changes_preserve_remaining_ticks_and_initial_reload_register() {
    let mut apic = LocalApic::new(0);
    apic.write(0x380, 100, 0).unwrap(); // Divide 2: deadline 2000 ns.
    apic.write(0x3e0, 3, 1000).unwrap(); // Fifty ticks remain; divide 16.
    assert_eq!(apic.read(0x380, 1000), Some(100));
    assert_eq!(apic.read(0x390, 1000), Some(50));
    assert_eq!(apic.read(0x390, 9000), Some(0));
}
