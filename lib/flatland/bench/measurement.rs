use crate::allocations::{ALLOCATED_BYTES, ALLOCATIONS};
use std::{sync::atomic::Ordering, time::Instant};
pub fn measure(name: &str, bytes_per_iteration: u64, mut operation: impl FnMut(usize)) {
    assert!(
        !cfg!(debug_assertions),
        "run benchmarks with bazel run -c opt"
    );
    for i in 0..10 {
        operation(i);
    }
    let mut times = [0u64; 200];
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    for (i, elapsed) in times.iter_mut().enumerate() {
        let begin = Instant::now();
        operation(i + 10);
        *elapsed = begin.elapsed().as_nanos() as u64;
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
    let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) - bytes;
    times.sort_unstable();
    println!(
        "{{\"workload\":\"{name}\",\"scope\":\"host operation wall time\",\"samples\":200,\"warmup\":10,\"p50_ns\":{},\"p99_ns\":{},\"allocations\":{allocations},\"allocated_bytes\":{bytes},\"output_bytes_per_iteration\":{bytes_per_iteration},\"over_3ms\":{},\"over_7_5ms\":{},\"over_8_333ms\":{},\"hardware_performance_verified\":false}}",
        times[99],
        times[197],
        times.iter().filter(|t| **t > 3_000_000).count(),
        times.iter().filter(|t| **t > 7_500_000).count(),
        times.iter().filter(|t| **t > 8_333_000).count()
    );
    assert_eq!(allocations, 0, "retained operation allocated after warmup");
}
