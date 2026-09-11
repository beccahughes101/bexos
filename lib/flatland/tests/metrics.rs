use bexos_flatland::metrics::Window;
#[test]
fn rolling_percentiles_discard_old_outliers_and_use_nearest_rank() {
    let mut w = Window::default();
    assert_eq!(w.percentile(99), None);
    w.record(u64::MAX);
    for i in 1..=256 {
        w.record(i);
    }
    assert_eq!(w.len(), 256);
    assert_eq!(w.percentile(50), Some(128));
    assert_eq!(w.percentile(99), Some(254));
    assert_eq!(w.percentile(100), Some(256));
    assert_eq!(w.percentile(0), None);
    assert_eq!(w.percentile(101), None);
    w.record(0);
    assert_eq!(w.percentile(1), Some(3));
}
