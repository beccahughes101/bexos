use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::*;

global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .p2align 3
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .p2align 3
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .p2align 3
"#
);

#[unsafe(no_mangle)]
pub(super) static SECONDARY_BOOT_READY: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
pub(super) static CPU_BOOT_MASK: AtomicU64 = AtomicU64::new(1);
pub(super) static CONFIGURED_MAX_CPUS: AtomicU64 = AtomicU64::new(1);
pub(super) static SECONDARY_SCHED_READY: AtomicBool = AtomicBool::new(false);

// The transplant image is only entered on CPU0 and switches to its dedicated
// 4 MiB stack before Rust executes. Reserve one additional MiB for its guard
// space while keeping the image plus its post-takeover heap inside UPDATE_END.
// A cold-boot kernel still needs the full per-CPU stack allocation.
#[cfg(bexos_update_kernel)]
const BOOT_STACK_BYTES: usize = 5 * 1024 * 1024;
#[cfg(not(bexos_update_kernel))]
const BOOT_STACK_BYTES: usize = 64 * PER_CPU_STACK_STRIDE;
const PER_CPU_STACK_BYTES: usize = 256 * 1024;
const PER_CPU_STACK_STRIDE: usize = PER_CPU_STACK_BYTES + 4096;

#[unsafe(no_mangle)]
pub extern "C" fn secondary_kernel_main(cpu_id: u64) -> ! {
    set_kernel_thread_pointer(cpu_id);
    CPU_BOOT_MASK.fetch_or(1u64 << cpu_id, Ordering::AcqRel);
    crate::secondary_cpu_main(cpu_id)
}

global_asm!(
    r#"
    .macro save_simd
    stp q0, q1, [sp, #272]
    stp q2, q3, [sp, #304]
    stp q4, q5, [sp, #336]
    stp q6, q7, [sp, #368]
    stp q8, q9, [sp, #400]
    stp q10, q11, [sp, #432]
    stp q12, q13, [sp, #464]
    stp q14, q15, [sp, #496]
    stp q16, q17, [sp, #528]
    stp q18, q19, [sp, #560]
    stp q20, q21, [sp, #592]
    stp q22, q23, [sp, #624]
    stp q24, q25, [sp, #656]
    stp q26, q27, [sp, #688]
    stp q28, q29, [sp, #720]
    stp q30, q31, [sp, #752]
    mrs x0, fpcr
    str x0, [sp, #784]
    mrs x0, fpsr
    str x0, [sp, #792]
    mrs x0, tpidr_el0
    str x0, [sp, #800]
    mov x0, #1
    str x0, [sp, #808]
    .endm
    .macro restore_simd
    ldp q0, q1, [sp, #272]
    ldp q2, q3, [sp, #304]
    ldp q4, q5, [sp, #336]
    ldp q6, q7, [sp, #368]
    ldp q8, q9, [sp, #400]
    ldp q10, q11, [sp, #432]
    ldp q12, q13, [sp, #464]
    ldp q14, q15, [sp, #496]
    ldp q16, q17, [sp, #528]
    ldp q18, q19, [sp, #560]
    ldp q20, q21, [sp, #592]
    ldp q22, q23, [sp, #624]
    ldp q24, q25, [sp, #656]
    ldp q26, q27, [sp, #688]
    ldp q28, q29, [sp, #720]
    ldp q30, q31, [sp, #752]
    ldr x0, [sp, #784]
    msr fpcr, x0
    ldr x0, [sp, #792]
    msr fpsr, x0
    ldr x0, [sp, #800]
    msr tpidr_el0, x0
    .endm
    .section .bss.stack, "aw", %nobits
    .balign 16
    .global __boot_stacks_bottom
__boot_stacks_bottom:
    .skip {boot_stack_bytes}
    .global __boot_stacks_top
__boot_stacks_top:

    .section .text.boot, "ax"
    .balign 2048
    .global _start
_start:
    .inst 0xd503245f
    mov x20, x0
    // QEMU's virt firmware enters a raw -kernel payload above EL1.  With
    // secure=on that can be EL3; otherwise it is commonly EL2.  The kernel owns
    // EL1 page tables and exception state, so drop to EL1 before touching the
    // runtime state used by process entry and exception return.
    mrs x4, CurrentEL
    cmp x4, #0xc
    b.eq .Lboot_el3
    cmp x4, #0x8
    b.eq .Lboot_el2
    b .Lboot_el1
.Lboot_el3:
    mov x4, #0x501
    msr scr_el3, x4
    isb
    mov x4, #1
    lsl x4, x4, #31
    mov x5, #3
    lsl x5, x5, #40
    orr x4, x4, x5
    msr hcr_el2, x4
    mov x4, #3
    msr cnthctl_el2, x4
    msr cntvoff_el2, xzr
    mov x4, #0x3c5
    msr spsr_el3, x4
    adr x4, .Lboot_el1
    msr elr_el3, x4
    eret
.Lboot_el2:
    mov x4, #1
    lsl x4, x4, #31
    mov x5, #3
    lsl x5, x5, #40
    orr x4, x4, x5
    msr hcr_el2, x4
    mov x4, #3
    msr cnthctl_el2, x4
    msr cntvoff_el2, xzr
    mov x4, #0x3c5
    msr spsr_el2, x4
    adr x4, .Lboot_el1
    msr elr_el2, x4
    eret
.Lboot_el1:
    mrs x0, mpidr_el1
    and x0, x0, #0xff
    adrp x1, __boot_stacks_bottom
    add x1, x1, :lo12:__boot_stacks_bottom
    add x2, x0, #1
    mov x3, #65
    lsl x3, x3, #12
    mul x2, x2, x3
    add sp, x1, x2
    cbnz x0, 6f

    adrp x0, __exception_vectors
    add x0, x0, :lo12:__exception_vectors
    msr vbar_el1, x0
    isb

    mov x0, #3
    msr cntkctl_el1, x0
    mov x0, #(3 << 20)
    msr cpacr_el1, x0
    isb
    mrs x0, mpidr_el1
    and x0, x0, #0xff
    msr tpidr_el1, x0

    adrp x0, __bss_start
    add x0, x0, :lo12:__bss_start
    adrp x1, __bss_end
    add x1, x1, :lo12:__bss_end
    mov x2, xzr
4:
    cmp x0, x1
    b.hs 5f
    str x2, [x0], #8
    b 4b
5:
    adr x0, .Lsecondary_park_template
    mov x1, #0x2000
    movk x1, #0x4010, lsl #16
    mov x2, #(.Lsecondary_park_end - .Lsecondary_park_template)
13:
    ldr w3, [x0], #4
    str w3, [x1], #4
    subs x2, x2, #4
    b.ne 13b
    dsb sy
    ic iallu
    dsb sy
    isb
    adrp x0, SECONDARY_BOOT_READY
    add x0, x0, :lo12:SECONDARY_BOOT_READY
    mov x1, #1
    str x1, [x0]
    dsb sy
    sev
    mov x0, x20
    bl kernel_main
3:
    wfe
    b 3b
6:
    adrp x1, SECONDARY_BOOT_READY
    add x1, x1, :lo12:SECONDARY_BOOT_READY
7:
    ldr x2, [x1]
    cbnz x2, 8f
    wfe
    b 7b
8:
    // PSCI resets each CPU's EL1 controls independently. Establish the same
    // timer and FP/SIMD access as the primary before entering compiled Rust
    // (including the exception vectors, which save SIMD registers).
    mov x1, #3
    msr cntkctl_el1, x1
    mov x1, #(3 << 20)
    msr cpacr_el1, x1
    adrp x1, __exception_vectors
    add x1, x1, :lo12:__exception_vectors
    msr vbar_el1, x1
    isb
    bl secondary_kernel_main
9:
    wfe
    b 9b

.Lsecondary_park_template:
    msr daifset, #0xf
    mov x2, #1
    lsl x2, x2, x0
14:
    ldaxr x3, [x1]
    orr x3, x3, x2
    stlxr w4, x3, [x1]
    cbnz w4, 14b
15:
    wfe
    b 15b
.Lsecondary_park_end:

    .global kernel_transplant_entry
kernel_transplant_entry:
    .inst 0xd503245f
    msr daifset, #0xf
    mov x19, x0
    adrp x1, __boot_stacks_bottom
    add x1, x1, :lo12:__boot_stacks_bottom
    add sp, x1, #0x40, lsl #12
    mov x0, x19
    bl kernel_transplant_main
    b .

    .global enter_context
enter_context:
    .inst 0xd503245f
    sub sp, sp, #816
    mov x1, sp
    mov x2, #102
10:
    ldr x3, [x0], #8
    str x3, [x1], #8
    subs x2, x2, #1
    b.ne 10b
    b __restore_user_frame

    .balign 2048
    .global __exception_vectors
__exception_vectors:
    b __sync_current_sp0
    .balign 128
    b __irq_current_sp0
    .balign 128
    b __fiq_current_sp0
    .balign 128
    b __serror_current_sp0
    .balign 128
    b __sync_current_spx
    .balign 128
    b __irq_current_spx
    .balign 128
    b __fiq_current_spx
    .balign 128
    b __serror_current_spx
    .balign 128
    b __sync_lower_aarch64
    .balign 128
    b __irq_lower_aarch64
    .balign 128
    b __fiq_lower_aarch64
    .balign 128
    b __serror_lower_aarch64
    .balign 128
    b __sync_lower_aarch32
    .balign 128
    b __irq_lower_aarch32
    .balign 128
    b __fiq_lower_aarch32
    .balign 128
    b __serror_lower_aarch32

__sync_current_sp0:
    b __prepare_sync_fault
__fiq_current_sp0:
__serror_current_sp0:
__sync_current_spx:
    b __prepare_sync_fault
__fiq_current_spx:
__serror_current_spx:
__fiq_lower_aarch64:
__serror_lower_aarch64:
__sync_lower_aarch32:
__fiq_lower_aarch32:
__serror_lower_aarch32:
    b .

__prepare_sync_fault:
    adrp x0, PREPARE_RETURN_SP
    add x0, x0, :lo12:PREPARE_RETURN_SP
    ldr x0, [x0]
    cbz x0, .Lprepare_unhandled_fault
    mrs x1, esr_el1
    adrp x0, PREPARE_FAULT_ESR
    str x1, [x0, :lo12:PREPARE_FAULT_ESR]
    mrs x1, elr_el1
    adrp x0, PREPARE_FAULT_ELR
    str x1, [x0, :lo12:PREPARE_FAULT_ELR]
    mrs x1, far_el1
    adrp x0, PREPARE_FAULT_FAR
    str x1, [x0, :lo12:PREPARE_FAULT_FAR]
    b kernel_prepare_failure
.Lprepare_unhandled_fault:
    b .

    .global kernel_prepare_call
kernel_prepare_call:
    .inst 0xd503245f
    sub sp, sp, #192
    stp x19, x20, [sp, #0]
    stp x21, x22, [sp, #16]
    stp x23, x24, [sp, #32]
    stp x25, x26, [sp, #48]
    stp x27, x28, [sp, #64]
    stp x29, x30, [sp, #80]
    stp d8, d9, [sp, #96]
    stp d10, d11, [sp, #112]
    stp d12, d13, [sp, #128]
    stp d14, d15, [sp, #144]
    mrs x9, daif
    str x9, [sp, #160]
    mrs x9, fpcr
    str x9, [sp, #168]
    mrs x9, fpsr
    str x9, [sp, #176]
    adrp x9, PREPARE_RETURN_SP
    add x9, x9, :lo12:PREPARE_RETURN_SP
    mov x10, sp
    str x10, [x9]
    mov x16, x0
    mov sp, x1
    mov x0, x2
    mov x1, x3
    mov x2, x4
    msr daifclr, #2
    blr x16
    b __prepare_return
    .global kernel_prepare_failure
kernel_prepare_failure:
    .inst 0xd503245f
    mov x0, #-1
__prepare_return:
    msr daifset, #0xf
    adrp x9, PREPARE_RETURN_SP
    add x9, x9, :lo12:PREPARE_RETURN_SP
    ldr x10, [x9]
    mov sp, x10
    str xzr, [x9]
    ldp x19, x20, [sp, #0]
    ldp x21, x22, [sp, #16]
    ldp x23, x24, [sp, #32]
    ldp x25, x26, [sp, #48]
    ldp x27, x28, [sp, #64]
    ldp x29, x30, [sp, #80]
    ldp d8, d9, [sp, #96]
    ldp d10, d11, [sp, #112]
    ldp d12, d13, [sp, #128]
    ldp d14, d15, [sp, #144]
    ldr x9, [sp, #168]
    msr fpcr, x9
    ldr x9, [sp, #176]
    msr fpsr, x9
    ldr x9, [sp, #160]
    add sp, sp, #192
    msr daif, x9
    ret

__sync_lower_aarch64:
    sub sp, sp, #816
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    stp x4, x5, [sp, #16 * 2]
    stp x6, x7, [sp, #16 * 3]
    stp x8, x9, [sp, #16 * 4]
    stp x10, x11, [sp, #16 * 5]
    stp x12, x13, [sp, #16 * 6]
    stp x14, x15, [sp, #16 * 7]
    stp x16, x17, [sp, #16 * 8]
    stp x18, x19, [sp, #16 * 9]
    stp x20, x21, [sp, #16 * 10]
    stp x22, x23, [sp, #16 * 11]
    stp x24, x25, [sp, #16 * 12]
    stp x26, x27, [sp, #16 * 13]
    stp x28, x29, [sp, #16 * 14]
    str x30, [sp, #16 * 15]
    mrs x0, sp_el0
    str x0, [sp, #248]
    mrs x0, elr_el1
    str x0, [sp, #256]
    mrs x0, spsr_el1
    str x0, [sp, #264]
    save_simd
    mov x0, sp
    bl handle_sync_lower_aarch64
__restore_user_frame:
    ldr x0, [sp, #248]
    msr sp_el0, x0
    ldr x0, [sp, #256]
    msr elr_el1, x0
    ldr x0, [sp, #264]
    msr spsr_el1, x0
    restore_simd
    ldp x0, x1, [sp, #16 * 0]
    ldp x2, x3, [sp, #16 * 1]
    ldp x4, x5, [sp, #16 * 2]
    ldp x6, x7, [sp, #16 * 3]
    ldp x8, x9, [sp, #16 * 4]
    ldp x10, x11, [sp, #16 * 5]
    ldp x12, x13, [sp, #16 * 6]
    ldp x14, x15, [sp, #16 * 7]
    ldp x16, x17, [sp, #16 * 8]
    ldp x18, x19, [sp, #16 * 9]
    ldp x20, x21, [sp, #16 * 10]
    ldp x22, x23, [sp, #16 * 11]
    ldp x24, x25, [sp, #16 * 12]
    ldp x26, x27, [sp, #16 * 13]
    ldp x28, x29, [sp, #16 * 14]
    ldr x30, [sp, #16 * 15]
    add sp, sp, #816
    eret

__irq_current_sp0:
__irq_current_spx:
__irq_lower_aarch64:
__irq_lower_aarch32:
    sub sp, sp, #816
    stp x0, x1, [sp, #16 * 0]
    stp x2, x3, [sp, #16 * 1]
    stp x4, x5, [sp, #16 * 2]
    stp x6, x7, [sp, #16 * 3]
    stp x8, x9, [sp, #16 * 4]
    stp x10, x11, [sp, #16 * 5]
    stp x12, x13, [sp, #16 * 6]
    stp x14, x15, [sp, #16 * 7]
    stp x16, x17, [sp, #16 * 8]
    stp x18, x19, [sp, #16 * 9]
    stp x20, x21, [sp, #16 * 10]
    stp x22, x23, [sp, #16 * 11]
    stp x24, x25, [sp, #16 * 12]
    stp x26, x27, [sp, #16 * 13]
    stp x28, x29, [sp, #16 * 14]
    str x30, [sp, #16 * 15]
    mrs x0, sp_el0
    str x0, [sp, #248]
    mrs x0, elr_el1
    str x0, [sp, #256]
    mrs x0, spsr_el1
    str x0, [sp, #264]
    save_simd
    mov x0, sp
    bl handle_irq
    ldr x0, [sp, #248]
    msr sp_el0, x0
    ldr x0, [sp, #256]
    msr elr_el1, x0
    ldr x0, [sp, #264]
    msr spsr_el1, x0
    restore_simd
    ldp x0, x1, [sp, #16 * 0]
    ldp x2, x3, [sp, #16 * 1]
    ldp x4, x5, [sp, #16 * 2]
    ldp x6, x7, [sp, #16 * 3]
    ldp x8, x9, [sp, #16 * 4]
    ldp x10, x11, [sp, #16 * 5]
    ldp x12, x13, [sp, #16 * 6]
    ldp x14, x15, [sp, #16 * 7]
    ldp x16, x17, [sp, #16 * 8]
    ldp x18, x19, [sp, #16 * 9]
    ldp x20, x21, [sp, #16 * 10]
    ldp x22, x23, [sp, #16 * 11]
    ldp x24, x25, [sp, #16 * 12]
    ldp x26, x27, [sp, #16 * 13]
    ldp x28, x29, [sp, #16 * 14]
    ldr x30, [sp, #16 * 15]
    add sp, sp, #816
    eret

"#,
    boot_stack_bytes = const BOOT_STACK_BYTES,
);

#[unsafe(no_mangle)]
pub extern "C" fn handle_sync_lower_aarch64(frame: *mut TrapFrame) {
    super::context::handle_sync_exception(exception_syndrome(), exception_link(), frame.cast());
}

unsafe extern "C" {
    pub fn enter_context(context: *const bexos_kernel_core::runtime::Context) -> !;
    pub(super) static __exception_vectors: u8;
}
