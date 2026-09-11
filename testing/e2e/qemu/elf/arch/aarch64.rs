use std::sync::atomic::AtomicU64;

pub fn preserve_vectors(value: u64, ready: &AtomicU64, peer: &AtomicU64) {
    let observed: u64;
    let restored: u64;
    unsafe {
        core::arch::asm!(
            "fmov d31, {value}",
            "mov {observed}, #1",
            "stlr {observed}, [{ready}]",
            "2:",
            "ldar {observed}, [{peer}]",
            "cbnz {observed}, 3f",
            "subs {budget}, {budget}, #1",
            "b.ne 2b",
            "3:",
            "fmov {restored}, d31",
            value = in(reg) value,
            ready = in(reg) ready.as_ptr(),
            peer = in(reg) peer.as_ptr(),
            observed = out(reg) observed,
            restored = out(reg) restored,
            budget = inout(reg) 5_000_000u64 => _,
            out("v31") _,
            options(nostack),
        );
    }
    assert_eq!(
        observed, 1,
        "another thread must run without a cooperative yield"
    );
    assert_eq!(restored, value, "SIMD context was corrupted by preemption");
}
