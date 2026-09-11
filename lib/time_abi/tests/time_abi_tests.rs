use bexos_time_abi::{
    SlewError, SlewState, TimePageSnapshot, TimePageV1, nanos_to_ticks, ticks_to_nanos,
};

#[test]
fn counter_conversions_continue_past_the_u64_multiply_boundary() {
    const FREQUENCY_HZ: u64 = 62_500_000;
    const TICKS_AT_320_SECONDS: u64 = 20_000_000_000;
    const NANOS_AT_320_SECONDS: u64 = 320_000_000_000;

    assert_eq!(
        ticks_to_nanos(TICKS_AT_320_SECONDS, FREQUENCY_HZ),
        NANOS_AT_320_SECONDS
    );
    assert_eq!(
        nanos_to_ticks(NANOS_AT_320_SECONDS, FREQUENCY_HZ),
        TICKS_AT_320_SECONDS
    );
}

#[test]
fn positive_negative_partial_completed_and_replaced_slews() {
    let state = SlewState::new().adjust(1_000, 1_000_000, 500).unwrap();
    assert_eq!(state.realtime_at(1_000).unwrap(), 1_000);
    assert_eq!(state.realtime_at(1_001_000).unwrap(), 1_001_500);
    let materialized = state.materialized_at(2_001_000).unwrap();
    assert_eq!(materialized.realtime_offset_ns, 1_000);
    assert_eq!(materialized.slew_remaining_ns, 999_000);

    let replaced = materialized.adjust(2_001_000, -499_000, 500).unwrap();
    assert_eq!(replaced.slew_remaining_ns, 500_000);
    assert_eq!(replaced.slew_rate_ppm, 500);

    let completed = replaced.materialized_at(1_002_002_000).unwrap();
    assert_eq!(completed.slew_remaining_ns, 0);
    assert_eq!(completed.slew_rate_ppm, 0);

    let negative = completed.adjust(1_002_002_000, -1_000, -500).unwrap();
    assert!(
        negative.realtime_at(1_002_003_000).unwrap()
            >= completed.realtime_at(1_002_002_000).unwrap()
    );
}

#[test]
fn rate_validation_and_direction_are_enforced() {
    assert_eq!(
        SlewState::new().adjust(0, 1, 501),
        Err(SlewError::InvalidRate)
    );
    assert_eq!(
        SlewState::new().adjust(0, 1, -500),
        Err(SlewError::DirectionMismatch)
    );
}

#[test]
fn realtime_overflow_is_reported() {
    let state = SlewState {
        realtime_offset_ns: i64::MAX,
        ..SlewState::new()
    };
    assert_eq!(state.adjust(0, 1, 0), Err(SlewError::Overflow));
}

#[test]
fn time_page_seqlock_snapshots_convert_ticks() {
    let mut page = TimePageV1::zeroed();
    page.write_snapshot(TimePageSnapshot {
        base_ticks: 100,
        tick_frequency_hz: 1_000,
        base_monotonic_ns: 1_000_000,
        realtime_offset_ns: 2_000,
        slew_start_monotonic_ns: 1_000_000,
        slew_remaining_ns: 1_000,
        slew_rate_ppm: 500,
    });
    let snapshot = page.read_snapshot().unwrap();
    assert_eq!(snapshot.monotonic_from_ticks(101), 2_000_000);
    assert_eq!(snapshot.realtime_from_ticks(101).unwrap(), 2_002_500);
}
