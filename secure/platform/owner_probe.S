/* Secure-EL2 execution-owner proof. The immutable EL3 bootstrap relocates the
 * owner into secure-only RAM, installs independent stage-two views for two
 * S-EL1 payload banks, and then leaves all execution control to the owner. */
.section .text.entry,"ax"
.global _start
_start:
    mrs x0, CurrentEL
    cmp x0, #12
    b.ne bootstrap_failed
    adr x0, el3_vectors
    msr vbar_el3, x0

    /* Copy the permanent owner and two distinct payloads into secure-only RAM. */
    adr x0, owner_start
    adr x1, owner_end
    ldr x2, =BEXOS_OWNER_BASE
    bl copy_region
    adr x0, payload_a_start
    adr x1, payload_a_end
    ldr x2, =BEXOS_TRUSTY_A_BASE
    bl copy_region
    adr x0, payload_a_start
    adr x1, payload_a_end
    ldr x2, =0x0e000000
    bl copy_region
    adr x0, payload_b_start
    adr x1, payload_b_end
    ldr x2, =BEXOS_TRUSTY_B_BASE
    bl copy_region
    dsb sy
    ic iallu
    dsb sy
    isb

    /* Secure EL2 exists beneath EL3; HVC from S-EL1 is owned there. */
    ldr x0, =0x40500
    msr scr_el3, x0
    mov x0, #0x3c9
    msr spsr_el3, x0
    ldr x0, =BEXOS_OWNER_BASE
    msr elr_el3, x0
    eret

copy_region:
    cmp x0, x1
    b.hs 2f
1:  ldr x3, [x0], #8
    str x3, [x2], #8
    cmp x0, x1
    b.lo 1b
2:  ret

bootstrap_failed:
    adr x1, bootstrap_failure_text
    mov x0, #4
    hlt #0xf000
    adr x1, failure_exit
    mov x0, #0x20
    hlt #0xf000
    b .
bootstrap_failure_text: .asciz "secure-owner: bootstrap FAIL\n"
.balign 8
failure_exit: .quad 0x20026, 1

.balign 2048
el3_vectors:
.rept 16
    b bootstrap_failed
    .balign 128
.endr

/* Everything in this range is position independent after relocation. */
.balign 4096
owner_start:
    mrs x0, CurrentEL
    cmp x0, #8
    b.ne owner_failed
    adr x0, sel2_vectors
    msr vbar_el2, x0

    /* L0[0] -> L1[0] -> L2. The payload IPA lives below 1 GiB. */
    ldr x10, =BEXOS_OWNER_BASE + 0x10000
    ldr x11, =BEXOS_OWNER_BASE + 0x11000
    ldr x12, =BEXOS_OWNER_BASE + 0x12000
    mov x0, x10
    mov x1, #1536
3:  str xzr, [x0], #8
    subs x1, x1, #1
    b.ne 3b
    orr x0, x11, #3
    str x0, [x10]
    orr x0, x12, #3
    str x0, [x11]

    /* Map 64 MiB at IPA 0x0e000000 to bank A with 2 MiB blocks. */
    ldr x0, =BEXOS_TRUSTY_A_BASE
    add x1, x12, #(112 * 8)
    mov x2, #32
    mov x5, #0x4fd
4:  orr x3, x0, x5
    str x3, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 4b

    /* 40-bit IPA starting at level 0, 42-bit output PA, 4 KiB granule,
     * WBWA, inner shareable. VSTCR owns T0SZ and SL0 for the secure walk;
     * only the cacheability, shareability, and PA-size fields are shared. */
    ldr x0, =0x33598
    msr vtcr_el2, x0
    ldr x0, =0x80000098
    msr vstcr_el2, x0
    /* Program both roots. With stage one disabled, implementations may retain
     * either IPA security attribute; both views must resolve to the owner's
     * deliberately identical initial table. */
    msr vttbr_el2, x10
    msr vsttbr_el2, x10
    /* Enter the bootstrap payload without translation, then atomically enable
     * stage two with the candidate table at the replacement boundary. */
    ldr x0, =0x80000000
    msr hcr_el2, x0
    ldr x0, =0x30d00800
    msr sctlr_el1, x0
    mov x0, #0x3c5
    msr spsr_el2, x0
    ldr x0, =0x0e000000
    msr elr_el2, x0
    dsb sy
    isb
    eret

/* Lower-EL synchronous exceptions are the only admitted owner entry. */
.balign 2048
sel2_vectors:
.rept 8
    b owner_failed
    .balign 128
.endr
    b sel2_sync
    .balign 128
.rept 7
    b owner_failed
    .balign 128
.endr

sel2_sync:
    mrs x9, esr_el2
    lsr x10, x9, #26
    cmp x10, #0x16
    b.eq owner_replace
    cmp x10, #0x24
    b.eq peer_access_blocked
    b owner_failed

owner_replace:
    cmp x0, #1
    b.ne owner_failed
    /* Publish a complete table only after all candidate descriptors exist. */
    ldr x10, =BEXOS_OWNER_BASE + 0x13000
    ldr x11, =BEXOS_OWNER_BASE + 0x14000
    ldr x12, =BEXOS_OWNER_BASE + 0x15000
    mov x4, x10
    mov x1, #1536
5:  str xzr, [x4], #8
    subs x1, x1, #1
    b.ne 5b
    orr x4, x11, #3
    str x4, [x10]
    orr x4, x12, #3
    str x4, [x11]
    ldr x4, =BEXOS_TRUSTY_B_BASE
    add x1, x12, #(112 * 8)
    mov x2, #32
    mov x5, #0x4fd
6:  orr x3, x4, x5
    str x3, [x1], #8
    add x4, x4, #0x200000
    subs x2, x2, #1
    b.ne 6b
    dsb ishst
    msr vttbr_el2, x10
    msr vsttbr_el2, x10
    ldr x4, =0x80000001
    msr hcr_el2, x4
    dsb ish
    tlbi vmalls12e1is
    dsb ish
    isb
    ldr x0, =0x0e000000
    msr elr_el2, x0
    eret

peer_access_blocked:
    mrs x0, far_el2
    ldr x1, =BEXOS_TRUSTY_A_BASE
    cmp x0, x1
    b.ne owner_failed
    adr x1, owner_success_text
    mov x0, #4
    hlt #0xf000
    adr x1, success_exit
    mov x0, #0x20
    hlt #0xf000
    b .

owner_failed:
    adr x1, owner_failure_text
    mov x0, #4
    hlt #0xf000
    adr x1, owner_failure_exit
    mov x0, #0x20
    hlt #0xf000
    b .
owner_success_text: .asciz "secure-owner: S-EL1 bank replacement and peer isolation verified\n"
owner_failure_text: .asciz "secure-owner: execution FAIL\n"
.balign 8
success_exit: .quad 0x20026, 0
owner_failure_exit: .quad 0x20026, 1
.ltorg
.balign 8
owner_end:

.balign 8
payload_a_start:
    mov x0, #1
    hvc #0
    b .
.balign 8
payload_a_end:

.balign 8
payload_b_start:
    /* The candidate executes distinct code, then proves its stage-two view
     * cannot address the retired physical bank as an IPA. */
    /* Construct the address inline so this independently relocated payload
     * has no PC-relative reference back into the bootstrap image. */
    movz x0, #(BEXOS_TRUSTY_A_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 48) & 0xffff), lsl #48
    ldr x1, [x0]
    b .
.balign 8
payload_b_end:
