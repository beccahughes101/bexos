use bexos_d1_rtc::{Pl031, RegisterIo, RtcError, UTC_2020_SECONDS};

#[derive(Default)]
struct FakeIo {
    data: [u32; 4],
}

impl RegisterIo for FakeIo {
    fn read32(&self, offset: usize) -> u32 {
        self.data[offset / 4]
    }

    fn write32(&mut self, offset: usize, value: u32) {
        self.data[offset / 4] = value;
    }
}

#[test]
fn reads_valid_pl031_seconds_as_utc_nanoseconds() {
    let mut io = FakeIo::default();
    io.data[0] = UTC_2020_SECONDS + 7;
    let rtc = Pl031::new(io);
    assert_eq!(
        rtc.read_utc_ns().unwrap(),
        i64::from(UTC_2020_SECONDS + 7) * 1_000_000_000
    );
}

#[test]
fn rejects_pre_2020_and_out_of_range_values() {
    let mut rtc = Pl031::new(FakeIo::default());
    assert_eq!(rtc.read_utc_ns(), Err(RtcError::InvalidValue));
    assert_eq!(rtc.write_utc_ns(-1), Err(RtcError::InvalidValue));
    assert_eq!(
        rtc.write_utc_ns((i64::from(u32::MAX) + 1) * 1_000_000_000),
        Err(RtcError::OutOfRange)
    );
}

#[test]
fn writes_seconds_to_load_register() {
    let mut rtc = Pl031::new(FakeIo::default());
    rtc.write_utc_ns(i64::from(UTC_2020_SECONDS + 11) * 1_000_000_000 + 999)
        .unwrap();
    assert_eq!(rtc.into_inner().data[2], UTC_2020_SECONDS + 11);
}
