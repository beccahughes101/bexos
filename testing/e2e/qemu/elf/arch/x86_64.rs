use std::sync::atomic::AtomicU64;

pub fn preserve_vectors(value: u64, ready: &AtomicU64, peer: &AtomicU64) {
    let observed: u64;
    let restored: u64;
    unsafe {
        core::arch::asm!(
            "movq xmm15, {value}",
            "mov qword ptr [{ready}], 1",
            "2:",
            "mov {observed}, [{peer}]",
            "test {observed}, {observed}",
            "jnz 3f",
            "dec {budget}",
            "jnz 2b",
            "3:",
            "movq {restored}, xmm15",
            value = in(reg) value,
            ready = in(reg) ready.as_ptr(),
            peer = in(reg) peer.as_ptr(),
            observed = out(reg) observed,
            restored = out(reg) restored,
            budget = inout(reg) 5_000_000u64 => _,
            out("xmm15") _,
            options(nostack),
        );
    }
    assert_eq!(
        observed, 1,
        "another thread must run without a cooperative yield"
    );
    assert_eq!(restored, value, "SSE2 context was corrupted by preemption");
}
