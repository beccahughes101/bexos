//! The only call into replaceable code. No guest or device transaction is
//! active here. Recovery returns to this resident frame, never to a checkpoint
//! of guest state. Candidate stacks and FP state are private and disposable.
use core::arch::global_asm;
global_asm!(
    r#"
.section .resident.transfer_control,"aw",@nobits
.balign 16
policy_saved_rsp:
.skip 16
.section .resident.client,"aw",@nobits
.balign 64
policy_saved_fx:
.skip 512
.section .resident.text,"ax"
.global policy_guard_call
policy_guard_call:
    clgi
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rip + policy_saved_rsp], rsp
    fxsave64 [rip + policy_saved_fx]
    mov [rip + BEXOS_MONITOR_RECOVERY_DEADLINE], r8
    lea rax, [rip + policy_guard_abort]
    mov [rip + BEXOS_MONITOR_RECOVERY_ENTRY], rax
    mov rsp, 0x04200000
    stgi
    mov rax, 0x04000000
    call rax
    clgi
    jmp policy_guard_return
policy_guard_abort:
    cli
    clgi
    mov rax, -1
policy_guard_return:
    cld
    mov qword ptr [rip + BEXOS_MONITOR_RECOVERY_ENTRY], 0
    mov rsp, [rip + policy_saved_rsp]
    fxrstor64 [rip + policy_saved_fx]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
"#
);
unsafe extern "C" {
    fn policy_guard_call(header: u64, sequence: u64, now: u64, pending: u64, deadline: u64) -> u64;
}
pub unsafe fn call(sequence: u64, pending: bool, budget_ns: u64) -> u64 {
    let now = unsafe { bexos_secure_monitor::clock::now_ns() };
    let Some(deadline) = (unsafe { bexos_secure_monitor::clock::deadline_ticks(budget_ns) }) else {
        return bexos_secure_monitor::policy_image::FAILURE;
    };
    let result = unsafe {
        policy_guard_call(
            bexos_secure_monitor::policy_image::CALL,
            sequence,
            now,
            u64::from(pending),
            deadline,
        )
    };
    unsafe { bexos_secure_monitor::watchdog::service_pending() };
    if result == bexos_secure_monitor::policy_image::FAILURE {
        crate::log("monitor-runtime: scheduling policy call failed; elapsed ns=\n");
        crate::hex(unsafe { bexos_secure_monitor::clock::now_ns() }.saturating_sub(now));
        crate::log("monitor-runtime: scheduling policy call budget ns=\n");
        crate::hex(budget_ns);
    }
    result
}
