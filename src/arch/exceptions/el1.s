// Exception Numbers:
// 0: Synchronous from current SP0
// 1: IRQ from current SP0
// 2: FIQ from current SP0
// 3: SError from current SP0
// 4: Synchronous from current SPx
// 5: IRQ from current SPx
// 6: FIQ from current SPx
// 7: SError from current SPx

.align 11  // 2^11 = 2048
.global el1_exception_vector

el1_exception_vector:
    .align 7
    b sync_sp0   // 0
    .align 7 
    b irq_sp0    // 1
    .align 7
    b fiq_sp0    // 2
    .align 7
    b serr_sp0   // 3
    .align 7
    b sync_spx   // 4
    .align 7
    b irq_spx    // 5
    .align 7
    b fiq_spx    // 6
    .align 7
    b serr_spx   // 7

// Macro to save registers and call the Rust handler
.equ EXCEPTION_FRAME_SIZE, 288

.macro SAVE_AND_CALL_HANDLER exception_number
    // Keep SP 16-byte aligned.
    sub sp, sp, #EXCEPTION_FRAME_SIZE

    // Save original GPRs before using any as scratch registers.
    stp x0,  x1,  [sp, #(0*8)]
    stp x2,  x3,  [sp, #(2*8)]
    stp x4,  x5,  [sp, #(4*8)]
    stp x6,  x7,  [sp, #(6*8)]
    stp x8,  x9,  [sp, #(8*8)]
    stp x10, x11, [sp, #(10*8)]
    stp x12, x13, [sp, #(12*8)]
    stp x14, x15, [sp, #(14*8)]
    stp x16, x17, [sp, #(16*8)]
    stp x18, x19, [sp, #(18*8)]
    stp x20, x21, [sp, #(20*8)]
    stp x22, x23, [sp, #(22*8)]
    stp x24, x25, [sp, #(24*8)]
    stp x26, x27, [sp, #(26*8)]
    stp x28, x29, [sp, #(28*8)]
    str x30,      [sp, #(30*8)]

    mrs x3, spsr_el1
    str x3, [sp, #(31*8)]

    mrs x3, elr_el1
    str x3, [sp, #(32*8)]

    mrs x3, esr_el1
    str x3, [sp, #(33*8)]

    mrs x3, far_el1
    str x3, [sp, #(34*8)]

    // Rust arguments:
    // x0 = exception number
    // x1 = pre-exception SP
    // x2 = pointer to Registers
    mov x0, \exception_number
    add x1, sp, #EXCEPTION_FRAME_SIZE
    mov x2, sp

    bl el1_handler

    // Restore exception return state in case the Rust handler returns.
    ldr x3, [sp, #(31*8)]
    msr spsr_el1, x3

    ldr x3, [sp, #(32*8)]
    msr elr_el1, x3

    // Restore all GPRs.
    ldp x0,  x1,  [sp, #(0*8)]
    ldp x2,  x3,  [sp, #(2*8)]
    ldp x4,  x5,  [sp, #(4*8)]
    ldp x6,  x7,  [sp, #(6*8)]
    ldp x8,  x9,  [sp, #(8*8)]
    ldp x10, x11, [sp, #(10*8)]
    ldp x12, x13, [sp, #(12*8)]
    ldp x14, x15, [sp, #(14*8)]
    ldp x16, x17, [sp, #(16*8)]
    ldp x18, x19, [sp, #(18*8)]
    ldp x20, x21, [sp, #(20*8)]
    ldp x22, x23, [sp, #(22*8)]
    ldp x24, x25, [sp, #(24*8)]
    ldp x26, x27, [sp, #(26*8)]
    ldp x28, x29, [sp, #(28*8)]
    ldr x30,      [sp, #(30*8)]

    add sp, sp, #EXCEPTION_FRAME_SIZE
    eret
.endm

sync_sp0:
    SAVE_AND_CALL_HANDLER 0
irq_sp0:
    SAVE_AND_CALL_HANDLER 1
fiq_sp0:
    SAVE_AND_CALL_HANDLER 2
serr_sp0:
    SAVE_AND_CALL_HANDLER 3
sync_spx:
    SAVE_AND_CALL_HANDLER 4
irq_spx:
    SAVE_AND_CALL_HANDLER 5
fiq_spx:
    SAVE_AND_CALL_HANDLER 6
serr_spx:
    SAVE_AND_CALL_HANDLER 7