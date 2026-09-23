/* Bare firmware proof of S-EL2 availability and secure-only transition RAM.
 * This probe is not the Trusty execution owner or a live replacement test. */
.section .text.entry,"ax"
.global _start
_start:
    mrs x0, CurrentEL
    cmp x0, #12
    b.ne failed
    adr x0, el3_vectors
    msr vbar_el3, x0
    mrs x0, id_aa64pfr0_el1
    ubfx x0, x0, #36, #4
    cbz x0, failed
    ldr x0, =BEXOS_TRANSITION_BASE
    mov x1, #0x5a5a
    str x1, [x0]
    ldr x2, [x0]
    cmp x1, x2
    b.ne failed
    ldr x0, =(BEXOS_TRANSITION_BASE + BEXOS_TRANSITION_BYTES - 8)
    str x1, [x0]
    ldr x2, [x0]
    cmp x1, x2
    b.ne failed
    // Enter Secure EL2, then return to EL3 through its lower-EL SMC vector.
    ldr x0, =0x40400
    msr scr_el3, x0
    mov x0, #0x3c9
    msr spsr_el3, x0
    adr x0, sel2
    msr elr_el3, x0
    isb
    eret
sel2:
    mrs x0, CurrentEL
    cmp x0, #8
    b.ne failed
    smc #0
    b failed
el3_return:
    mrs x0, esr_el3
    lsr x0, x0, #26
    cmp x0, #0x17
    b.ne failed
    // Copy a position-independent probe and exception vectors to NS RAM.
    adr x0, ns_start
    adr x1, ns_end
    ldr x2, =0x40000000
copy_ns:
    ldr x3, [x0], #8
    str x3, [x2], #8
    cmp x0, x1
    b.lo copy_ns
    dsb sy
    ic iallu
    dsb sy
    isb
    ldr x0, =0x501
    msr scr_el3, x0
    isb
    // NS EL2 controls the execution state of NS EL1 even when EL2 is skipped.
    mov x0, #0x80000000
    msr hcr_el2, x0
    ldr x0, =0x30d00800
    msr sctlr_el1, x0
    mov x0, #0x3c5
    msr spsr_el3, x0
    ldr x0, =0x40000000
    msr elr_el3, x0
    isb
    eret
failed:
    adr x1, failure_text
    mov x0, #4
    hlt #0xf000
    adr x1, failure_exit
    mov x0, #0x20
    hlt #0xf000
    b .
failure_text: .asciz "secure-platform: FAIL\n"
.balign 8
failure_exit: .quad 0x20026, 1
.ltorg
.balign 2048
el3_vectors:
.rept 8
    b failed
    .balign 128
.endr
    b el3_return
    .balign 128
.rept 7
    b failed
    .balign 128
.endr
.balign 2048
ns_start:
    adr x0, ns_vectors
    msr vbar_el1, x0
    isb
    ldr x0, =BEXOS_TRANSITION_BASE
    ldr x1, [x0]
    b ns_failed
ns_abort:
    mrs x0, esr_el1
    lsr x0, x0, #26
    cmp x0, #0x25
    b.ne ns_failed
    mrs x0, far_el1
    ldr x1, =BEXOS_TRANSITION_BASE
    cmp x0, x1
    b.ne ns_failed
    adr x1, ns_success_text
    mov x0, #4
    hlt #0xf000
    adr x1, ns_success_exit
    mov x0, #0x20
    hlt #0xf000
    b .
ns_failed:
    adr x1, ns_failure_exit
    mov x0, #0x20
    hlt #0xf000
    b .
ns_success_text: .asciz "secure-platform: SEL2 and secure transition memory verified\n"
.balign 8
ns_success_exit: .quad 0x20026, 0
ns_failure_exit: .quad 0x20026, 1
.ltorg
.balign 2048
ns_vectors:
.rept 4
    b ns_failed
    .balign 128
.endr
    b ns_abort
    .balign 128
.rept 11
    b ns_failed
    .balign 128
.endr
.balign 8
ns_end:
