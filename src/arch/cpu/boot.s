.section ".text.boot"  // Make sure the linker puts this at the start of the kernel image

.global _start  // Execution starts here

_start:
    b       el2_start        // code0: jump over the header
    .long   0                 // code1
    .quad   0x80000           // text_offset: load offset from start of RAM... 
    .quad   __kernel_size     // image size (can be 0 if you don't have the symbol)
    .quad   2                 // flags: little-endian, 4K pages
    .quad   0                 // reserved
    .quad   0
    .quad   0
    .ascii  "ARM\x64"         // magic
    .long   0                 // reserved

el2_start:

    // Identify the hardware CPU from MPIDR_EL1.Aff0.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbz     x1, .Lprimary_cpu

    // Secondary CPUs are already executing this kernel image. Keep them in a
    // stackless WFE loop until CPU0 publishes a release bit and executes SEV.
    // SECONDARY_RELEASE_MASK lives in .data (not .bss), so its initial value is
    // valid even before CPU0 clears the BSS section.
.Lsecondary_wait:
    ldr     x2, =SECONDARY_RELEASE_MASK
    ldar    x3, [x2]
    mov     x4, #1
    lsl     x4, x4, x1
    tst     x3, x4
    b.ne    .Lsecondary_released

    wfe
    b       .Lsecondary_wait

.Lsecondary_released:
    // Give every released CPU a private EL2 transition stack before any BL.
    cmp     x1, #1
    b.eq    .Lsecondary_stack_cpu1
    cmp     x1, #2
    b.eq    .Lsecondary_stack_cpu2
    cmp     x1, #3
    b.eq    .Lsecondary_stack_cpu3
    b       park

.Lsecondary_stack_cpu1:
    ldr     x2, =__el2_stack_cpu1_top
    b       .Lsecondary_stack_ready
.Lsecondary_stack_cpu2:
    ldr     x2, =__el2_stack_cpu2_top
    b       .Lsecondary_stack_ready
.Lsecondary_stack_cpu3:
    ldr     x2, =__el2_stack_cpu3_top

.Lsecondary_stack_ready:
    mov     sp, x2
    bl      el1_drop
    b       park

.Lprimary_cpu:
    // Check if in EL2
    mrs     x1, CurrentEL
    and     x1, x1, 2
    cbz     x1, .Lprimary_stack

.Lprimary_stack:
    ldr     x1, =__el2_stack_cpu0_top
    mov     sp, x1

    // CPU0 alone owns global C runtime initialization.
    ldr     x1, =__bss_start
    ldr     w2, =__bss_size
.Lclear_bss:
    cbz     w2, .Lbss_ready
    str     xzr, [x1], #8
    sub     w2, w2, #1
    cbnz    w2, .Lclear_bss

.Lbss_ready:
    adrp    x1, ARCH_TIMER_COUNTER_FREQUENCY
    add     x1, x1, #:lo12:ARCH_TIMER_COUNTER_FREQUENCY
    mrs     x2, CNTFRQ_EL0
    cmp     x2, xzr
    b.eq    park
    str     w2, [x1]

    // CPU0 follows the same EL2 -> EL1 transition helper as secondaries.
    bl      el1_drop
    b       park
