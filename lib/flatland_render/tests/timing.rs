use bexos_flatland_render::timing::elapsed_ns;

#[test]
fn software_recovery_configuration_cannot_relax_hardware_deadlines() {
    use bexos_flatland_render::recovery::timeout_us;
    for millis in [2000, 5000, 30000] {
        assert_eq!(timeout_us(false, millis), Some(2_000_000));
        assert_eq!(timeout_us(true, millis), Some(u64::from(millis) * 1000));
    }
    for millis in [0, 1999, 30001, u32::MAX] {
        assert_eq!(timeout_us(true, millis), None);
        assert_eq!(timeout_us(false, millis), None);
    }
}

#[test]
fn timestamp_conversion_checks_counter_order_and_period() {
    let counters = |a: u64, b: u64| [a.to_le_bytes(), b.to_le_bytes()].concat();
    assert_eq!(elapsed_ns(&counters(100, 104), 2.5), Some(10));
    assert_eq!(elapsed_ns(&counters(100, 100), 1.), Some(0));
    assert_eq!(elapsed_ns(&counters(u64::MAX - 3, 2), 1.), None);
    assert_eq!(elapsed_ns(&counters(0, u64::MAX), 2.), None);
    assert_eq!(elapsed_ns(&[0; 15], 1.), None);
    for period in [0., -1., f32::NAN, f32::INFINITY] {
        assert_eq!(elapsed_ns(&counters(100, 104), period), None);
    }
}
