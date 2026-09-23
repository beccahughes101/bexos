/* Permanent ARM64 Trusty execution owner.
 *
 * TF-A authenticates this complete BL32 container and enters it at S-EL2.
 * The bootstrap relocates the owner above the S-EL1 IPA width, copies the
 * embedded Trusty image into bank A, installs secure stage two, and mediates
 * every SMC made by Trusty. The embedded payload path is supplied by Bazel.
 */
.section .text.entry,"ax"
.global _start
_start:
    /* Keep this as the first instruction: the pinned Trusty SPD recognizes
     * the standard AArch64 MRS encoding before selecting its entry mode. */
    mrs x9, CurrentEL
    cmp x9, #8
    b.ne boot_failed
    mov x20, x0
    mov x21, x1
    mov x22, x2
    mov x23, x3
    mrs x9, mpidr_el1
    and x9, x9, #0xff
    cbnz x9, secondary_boot

    adrp x0, owner_start
    add x0, x0, :lo12:owner_start
    adrp x1, owner_copy_end
    add x1, x1, :lo12:owner_copy_end
    ldr x2, =BEXOS_OWNER_BASE
    bl copy_region
    /* Static Rust state lives in NOLOAD BSS. Clear it explicitly so reset and
     * warm-boot behavior does not depend on the emulator's initial RAM fill. */
    adrp x0, owner_start
    add x0, x0, :lo12:owner_start
    adrp x1, owner_copy_end
    add x1, x1, :lo12:owner_copy_end
    sub x2, x1, x0
    ldr x0, =BEXOS_OWNER_BASE
    add x2, x0, x2
    adrp x0, owner_end
    add x0, x0, :lo12:owner_end
    sub x1, x0, x1
    cbz x1, 2f
1:  strb wzr, [x2], #1
    subs x1, x1, #1
    b.ne 1b
2:
    adrp x0, trusty_start
    add x0, x0, :lo12:trusty_start
    adrp x1, trusty_end
    add x1, x1, :lo12:trusty_end
    ldr x2, =BEXOS_TRUSTY_A_BASE
    bl copy_region
    dsb sy
    ic iallu
    dsb sy
    isb
    ldr x9, =BEXOS_OWNER_BASE
    br x9

secondary_boot:
    /* The primary completes initialization before TF-A admits secondaries. */
    ldr x9, =BEXOS_OWNER_BASE + (owner_secondary - owner_start)
    br x9

copy_region:
    cmp x0, x1
    b.hs 2f
1:  ldr x3, [x0], #8
    str x3, [x2], #8
    cmp x0, x1
    b.lo 1b
2:  ret

boot_failed:
    b boot_failed
.ltorg

/* This range is copied verbatim to permanent secure-only memory. */
.section .text.owner,"ax"
.balign 4096
owner_start:
    mrs x9, mpidr_el1
    and x9, x9, #0xff
    cbnz x9, owner_secondary
    adr x9, boot_arguments
    stp x20, x21, [x9]
    stp x22, x23, [x9, #16]

    /* L0[0:1] cover the 40-bit S-EL1 IPA space. L1[0] descends to L2 so
     * the Trusty window can be redirected while all other low addresses keep
     * identity mappings for the GIC, UART, and shared-memory transports. */
    ldr x10, =BEXOS_OWNER_BASE + 0x10000
    ldr x11, =BEXOS_OWNER_BASE + 0x11000
    ldr x12, =BEXOS_OWNER_BASE + 0x12000
    ldr x13, =BEXOS_OWNER_BASE + 0x13000
    mov x0, x10
    mov x1, #(4 * 512)
3:  str xzr, [x0], #8
    subs x1, x1, #1
    b.ne 3b
    orr x0, x11, #3
    str x0, [x10]
    orr x0, x13, #3
    str x0, [x10, #8]
    orr x0, x12, #3
    str x0, [x11]

    /* Low 1 GiB uses 2 MiB identity blocks except Trusty's real TF-A load
     * arena, 0x0e200000..0x0f000000. The payload binary starts at bank
     * offset zero, so its linked entry IPA maps to the first bank block. */
    mov x0, xzr
    mov x1, x12
    mov x2, #512
    mov x3, #0x4fd
4:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 4b
    /* Keep this value inline. The bootstrap also references bank A; allowing
     * the assembler to merge literal pools would leave a relocated load
     * pointing back outside the resident owner copy. */
    movz x0, #(BEXOS_TRUSTY_A_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 48) & 0xffff), lsl #48
    add x1, x12, #(113 * 8)
    mov x2, #7
5:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 5b

    /* Remaining L1 blocks are identity mapped in both 512 GiB halves. */
    mov x0, #0x40000000
    mov x5, #0x40000000
    add x1, x11, #8
    mov x2, #511
6:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, x5
    subs x2, x2, #1
    b.ne 6b
    mov x0, #0
    movk x0, #0x80, lsl #32
    mov x1, x13
    mov x2, #512
7:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, x5
    subs x2, x2, #1
    b.ne 7b
    /* Authenticate and recover the durable Trusty selection before the first
     * S-EL1 instruction. The Rust owner loads a non-embedded image into its
     * recorded bank and marks interrupted trials for rollback. */
    ldr x0, =BEXOS_OWNER_BASE + 0x90000
    mov sp, x0
    bl bexos_arm_owner_boot_select
    cbz x0, owner_failed
    mov x6, x0
    and x5, x0, #0xff
    cmp x5, #1
    b.eq 9f
    cmp x5, #2
    b.ne owner_failed
    ldr x12, =BEXOS_OWNER_BASE + 0x12000
    movz x0, #(BEXOS_TRUSTY_B_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 48) & 0xffff), lsl #48
    add x1, x12, #(113 * 8)
    mov x2, #7
    mov x3, #0x4fd
8:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 8b
9:
    /* Bit 8 identifies a pending image promoted to a reboot trial. */
    tst x6, #(1 << 8)
    adr x1, candidate_booting
    cset x2, ne
    str x2, [x1]
    dsb ishst

owner_secondary:
    adr x0, owner_vectors
    msr vbar_el2, x0
    /* Give each secure CPU a private permanent stack. */
    mrs x9, mpidr_el1
    and x9, x9, #0xff
    add x9, x9, #1
    lsl x9, x9, #16
    ldr x0, =BEXOS_OWNER_BASE + 0x80000
    add x0, x0, x9
    mov sp, x0

    ldr x10, =BEXOS_OWNER_BASE + 0x10000
    ldr x0, =0x33598
    msr vtcr_el2, x0
    ldr x0, =0x80000098
    msr vstcr_el2, x0
    msr vttbr_el2, x10
    msr vsttbr_el2, x10
    /* S-EL1 cannot use CNTPS_* when FEAT_SEL2 is enabled. Admit reads of the
     * physical counter and programming of CNTP_*; the owner still controls
     * routing and can mask the timer during a replacement boundary. */
    mov x0, #3
    msr cnthctl_el2, x0
    /* VM enables stage two, TSC routes Trusty's SMC exits through the owner,
     * and RW keeps the payload in AArch64 state. */
    ldr x0, =0x80080001
    msr hcr_el2, x0
    tlbi vmalls12e1is
    dsb ish
    isb
    adr x9, boot_arguments
    ldp x0, x1, [x9]
    ldp x2, x3, [x9, #16]
    mov x9, #0x3c5
    msr spsr_el2, x9
    ldr x9, =0x0e200000
    msr elr_el2, x9
    eret

.balign 2048
owner_vectors:
.rept 8
    b owner_failed
    .balign 128
.endr
    b lower_sync
    .balign 128
.rept 7
    b owner_failed
    .balign 128
.endr

.macro save_nested_el1 base
    mrs x16, ttbr0_el1
    mrs x17, ttbr1_el1
    stp x16, x17, [\base, #232]
    mrs x16, tcr_el1
    mrs x17, mair_el1
    stp x16, x17, [\base, #248]
    mrs x16, amair_el1
    mrs x17, vbar_el1
    stp x16, x17, [\base, #264]
    mrs x16, contextidr_el1
    mrs x17, tpidr_el1
    stp x16, x17, [\base, #280]
    mrs x16, tpidr_el0
    mrs x17, tpidrro_el0
    stp x16, x17, [\base, #296]
    mrs x16, cntkctl_el1
    mrs x17, cpacr_el1
    stp x16, x17, [\base, #312]
    mrs x16, sp_el0
    mrs x17, sp_el1
    stp x16, x17, [\base, #328]
    mrs x16, elr_el1
    mrs x17, spsr_el1
    stp x16, x17, [\base, #344]
    mrs x16, cntp_cval_el0
    mrs x17, cntp_ctl_el0
    stp x16, x17, [\base, #360]
    mrs x16, sctlr_el1
    mrs x17, csselr_el1
    stp x16, x17, [\base, #376]
    mrs x16, actlr_el1
    mrs x17, esr_el1
    stp x16, x17, [\base, #392]
    mrs x16, far_el1
    mrs x17, afsr0_el1
    stp x16, x17, [\base, #408]
    mrs x16, afsr1_el1
    str x16, [\base, #424]
.endm

.macro restore_nested_el1 base
    ldp x16, x17, [\base, #232]
    msr ttbr0_el1, x16
    msr ttbr1_el1, x17
    ldp x16, x17, [\base, #248]
    msr tcr_el1, x16
    msr mair_el1, x17
    ldp x16, x17, [\base, #264]
    msr amair_el1, x16
    msr vbar_el1, x17
    ldp x16, x17, [\base, #280]
    msr contextidr_el1, x16
    msr tpidr_el1, x17
    ldp x16, x17, [\base, #296]
    msr tpidr_el0, x16
    msr tpidrro_el0, x17
    ldp x16, x17, [\base, #312]
    msr cntkctl_el1, x16
    msr cpacr_el1, x17
    ldp x16, x17, [\base, #328]
    msr sp_el0, x16
    msr sp_el1, x17
    ldp x16, x17, [\base, #344]
    msr elr_el1, x16
    msr spsr_el1, x17
    ldp x16, x17, [\base, #360]
    msr cntp_cval_el0, x16
    msr cntp_ctl_el0, x17
    ldp x16, x17, [\base, #376]
    msr sctlr_el1, x16
    msr csselr_el1, x17
    ldp x16, x17, [\base, #392]
    msr actlr_el1, x16
    msr esr_el1, x17
    ldp x16, x17, [\base, #408]
    msr far_el1, x16
    msr afsr0_el1, x17
    ldr x16, [\base, #424]
    msr afsr1_el1, x16
    isb
.endm

.macro outer_world_switch
    /* The pinned Trusty SPD predates S-EL2 and therefore does not include
     * SP_EL2 in its secure context. Preserve the resident owner's private
     * stack independently while normal-world EL2 is running. */
    adr x16, owner_smc_stacks
    mrs x17, mpidr_el1
    and x17, x17, #0xff
    mov x15, sp
    str x15, [x16, x17, lsl #3]
    smc #0
    adr x16, owner_smc_stacks
    mrs x17, mpidr_el1
    and x17, x17, #0xff
    ldr x17, [x16, x17, lsl #3]
    mov sp, x17
.endm

lower_sync:
    /* TF-A's direct Trusty entry returns x0-x7 from the opposite world and
     * preserves x8-x30. Save that non-result set, plus the trapped return
     * state, while the S-EL2 owner performs the physical SMC on Trusty's
     * behalf. x4-x7 are retained in the frame only for source diagnostics;
     * an ordinary return must keep the values supplied by TF-A. */
    sub sp, sp, #432
    stp x4, x5, [sp]
    stp x6, x7, [sp, #16]
    stp x8, x9, [sp, #32]
    stp x10, x11, [sp, #48]
    stp x12, x13, [sp, #64]
    stp x14, x15, [sp, #80]
    stp x16, x17, [sp, #96]
    stp x18, x19, [sp, #112]
    stp x20, x21, [sp, #128]
    stp x22, x23, [sp, #144]
    stp x24, x25, [sp, #160]
    stp x26, x27, [sp, #176]
    stp x28, x29, [sp, #192]
    str x30, [sp, #208]
    mrs x16, elr_el2
    mrs x17, spsr_el2
    stp x16, x17, [sp, #216]
    save_nested_el1 sp
    mrs x16, esr_el2
    lsr x16, x16, #26
    cmp x16, #0x17
    b.ne owner_failed
    adr x17, candidate_booting
    ldr x16, [x17]
    cbz x16, 1f
    /* A freshly booted Trusty first yields only after its kernel and built-in
     * tasks are initialized. Complete the bank transition at that boundary. */
    movz x16, #0
    movk x16, #0x3c00, lsl #16
    cmp x0, x16
    /* Candidate initialization uses ordinary platform SMCs for the secure
     * console, entropy, RPMB, and CPU bring-up before yielding its first
     * NS_RETURN. Proxy those calls exactly like the source and keep the
     * candidate in booting state until the actual ready boundary arrives. */
    b.ne 1f
    str xzr, [x17]
    bl bexos_arm_owner_candidate_ready
    cbz x0, owner_failed
    mov x2, x0
    mov x1, xzr
    movz x0, #0
    movk x0, #0x3c00, lsl #16
    b 1f
    /* A trapped SMC is restarted at EL3 by the owner. TF-A returns here with
     * either the next normal-world Trusty request or the private BexOS
     * firmware envelope. Keep consuming owner calls without exposing them to
     * S-EL1; ordinary Trusty calls resume at their architectural return PC. */
1:
    outer_world_switch
    /* TF-A owns the outer secure/non-secure transition. Restore the nested
     * Trusty context before another owner call can cause TF-A to save it. */
    restore_nested_el1 sp
    adr x17, pending_restore
    ldr x16, [x17]
    cbnz x16, restore_source
    movz x16, #0xbe00
    movk x16, #0xfb00, lsl #16
    cmp x0, x16
    b.ne 2f
    sub sp, sp, #64
    stp x0, x1, [sp]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    mov x0, sp
    bl bexos_arm_owner_firmware
    mov x17, x0
    ldp x1, x2, [sp]
    add sp, sp, #64
    cmp x17, #1
    b.eq launch_candidate
    cmp x17, #2
    b.eq launch_candidate
    cmp x17, #3
    b.eq commit_response
    cmp x17, #4
    b.lo 6f
    cmp x17, #5
    b.hi owner_failed
    adr x16, pending_restore
    str x17, [x16]
    b 6f
commit_response:
    adr x16, source_saved
    str xzr, [x16]
6:
    movz x0, #0
    movk x0, #0x3c00, lsl #16
    b 1b
2:
    /* HCR_EL2.TSC reports the trapped SMC instruction. EL3 completed that
     * call on Trusty's behalf, so resume at the following instruction. */
    ldp x16, x17, [sp, #216]
    add x16, x16, #4
    msr elr_el2, x16
    msr spsr_el2, x17
restore_general:
    ldp x8, x9, [sp, #32]
    ldp x10, x11, [sp, #48]
    ldp x12, x13, [sp, #64]
    ldp x14, x15, [sp, #80]
    ldp x16, x17, [sp, #96]
    ldp x18, x19, [sp, #112]
    ldp x20, x21, [sp, #128]
    ldp x22, x23, [sp, #144]
    ldp x24, x25, [sp, #160]
    ldp x26, x27, [sp, #176]
    ldp x28, x29, [sp, #192]
    ldr x30, [sp, #208]
    add sp, sp, #432
    eret

launch_candidate:
    /* Keep the source's suspended call frame and complete architectural EL1
     * context until candidate service probes and persistent commitment finish. */
    adr x0, source_frame
    mov x1, sp
    str x1, [x0]
    adr x0, source_system
    mrs x1, ttbr0_el1
    mrs x2, ttbr1_el1
    stp x1, x2, [x0]
    mrs x1, tcr_el1
    mrs x2, mair_el1
    stp x1, x2, [x0, #16]
    mrs x1, amair_el1
    mrs x2, vbar_el1
    stp x1, x2, [x0, #32]
    mrs x1, contextidr_el1
    mrs x2, tpidr_el1
    stp x1, x2, [x0, #48]
    mrs x1, tpidr_el0
    mrs x2, tpidrro_el0
    stp x1, x2, [x0, #64]
    mrs x1, cntkctl_el1
    mrs x2, cpacr_el1
    stp x1, x2, [x0, #80]
    mrs x1, sp_el0
    mrs x2, sp_el1
    stp x1, x2, [x0, #96]
    mrs x1, elr_el1
    mrs x2, spsr_el1
    stp x1, x2, [x0, #112]
    mrs x1, cntp_cval_el0
    mrs x2, cntp_ctl_el0
    stp x1, x2, [x0, #128]
    mrs x1, sctlr_el1
    ldr x2, [sp, #216]
    add x2, x2, #4
    stp x1, x2, [x0, #144]
    ldr x1, [sp, #224]
    str x1, [x0, #160]
    adr x0, source_saved
    mov x1, #1
    str x1, [x0]
    /* Redirect the seven 2 MiB Trusty IPA blocks to the authenticated
     * inactive bank and invalidate all S-EL1 stage-two translations. */
    cmp x17, #1
    b.eq 3f
    cmp x17, #2
    b.ne owner_failed
    movz x0, #(BEXOS_TRUSTY_B_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 48) & 0xffff), lsl #48
    b 4f
3:
    movz x0, #(BEXOS_TRUSTY_A_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 48) & 0xffff), lsl #48
4:
    ldr x1, =BEXOS_OWNER_BASE + 0x12000 + (113 * 8)
    mov x2, #7
    mov x3, #0x4fd
5:  orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 5b
    dsb ishst
    tlbi vmalls12e1is
    dsb ish
    isb

    /* Restore the architectural S-EL1 reset contract for the independent
     * candidate context. Its address spaces, exception vectors and timers
     * cannot inherit values from the retired source instance. */
    ldr x0, =0x30d00800
    msr sctlr_el1, x0
    msr ttbr0_el1, xzr
    msr ttbr1_el1, xzr
    msr tcr_el1, xzr
    msr mair_el1, xzr
    msr amair_el1, xzr
    msr vbar_el1, xzr
    msr contextidr_el1, xzr
    msr tpidr_el1, xzr
    msr tpidr_el0, xzr
    msr tpidrro_el0, xzr
    msr cntkctl_el1, xzr
    msr cpacr_el1, xzr
    adr x0, candidate_booting
    mov x1, #1
    str x1, [x0]
    adr x9, boot_arguments
    ldp x0, x1, [x9]
    ldp x2, x3, [x9, #16]
    mov x9, #0x3c5
    msr spsr_el2, x9
    ldr x9, =0x0e200000
    msr elr_el2, x9
    isb
    eret

owner_failed:
    adr x0, source_saved
    ldr x0, [x0]
    cbz x0, 9f
    bl bexos_arm_owner_candidate_failed
    cmp x0, #4
    b.lo 9f
    cmp x0, #5
    b.hi 9f
    adr x1, pending_restore
    str x0, [x1]
    movz x0, #0
    movk x0, #0x3c00, lsl #16
    mov x1, #-2
    mrs x2, esr_el2
    mrs x3, elr_el2
    mrs x4, far_el2
    mrs x5, mpidr_el1
    outer_world_switch
    adr x0, pending_restore
    ldr x16, [x0]
    b restore_source
9:
    b owner_failed

restore_source:
    /* x16 is 4 for source bank A and 5 for source bank B. */
    cmp x16, #4
    b.eq 10f
    movz x0, #(BEXOS_TRUSTY_B_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_B_BASE >> 48) & 0xffff), lsl #48
    b 11f
10:
    movz x0, #(BEXOS_TRUSTY_A_BASE & 0xffff)
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 16) & 0xffff), lsl #16
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 32) & 0xffff), lsl #32
    movk x0, #((BEXOS_TRUSTY_A_BASE >> 48) & 0xffff), lsl #48
11:
    ldr x1, =BEXOS_OWNER_BASE + 0x12000 + (113 * 8)
    mov x2, #7
    mov x3, #0x4fd
12: orr x4, x0, x3
    str x4, [x1], #8
    add x0, x0, #0x200000
    subs x2, x2, #1
    b.ne 12b
    dsb ishst
    tlbi vmalls12e1is
    dsb ish
    isb
    adr x0, source_system
    ldp x1, x2, [x0]
    msr ttbr0_el1, x1
    msr ttbr1_el1, x2
    ldp x1, x2, [x0, #16]
    msr tcr_el1, x1
    msr mair_el1, x2
    ldp x1, x2, [x0, #32]
    msr amair_el1, x1
    msr vbar_el1, x2
    ldp x1, x2, [x0, #48]
    msr contextidr_el1, x1
    msr tpidr_el1, x2
    ldp x1, x2, [x0, #64]
    msr tpidr_el0, x1
    msr tpidrro_el0, x2
    ldp x1, x2, [x0, #80]
    msr cntkctl_el1, x1
    msr cpacr_el1, x2
    ldp x1, x2, [x0, #96]
    msr sp_el0, x1
    msr sp_el1, x2
    ldp x1, x2, [x0, #112]
    msr elr_el1, x1
    msr spsr_el1, x2
    ldp x1, x2, [x0, #128]
    msr cntp_cval_el0, x1
    msr cntp_ctl_el0, x2
    ldp x1, x2, [x0, #144]
    msr sctlr_el1, x1
    msr elr_el2, x2
    ldr x1, [x0, #160]
    msr spsr_el2, x1
    adr x0, source_frame
    ldr x0, [x0]
    mov sp, x0
    adr x0, source_saved
    str xzr, [x0]
    adr x0, pending_restore
    str xzr, [x0]
    adr x0, candidate_booting
    str xzr, [x0]
    mov x0, #-5
    mov x1, xzr
    mov x2, xzr
    mov x3, xzr
    b restore_general
.ltorg
.balign 8
owner_assembly_end:
.balign 8
boot_arguments:
    .space 32
candidate_booting:
    .quad 0
source_saved:
    .quad 0
pending_restore:
    .quad 0
source_frame:
    .quad 0
owner_smc_stacks:
    .space 32
.balign 16
source_system:
    .space 176

.section .trusty,"a"
.balign 4096
trusty_start:
.incbin TRUSTY_IMAGE
.balign 8
trusty_end:
