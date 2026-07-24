.section ".text.boot"  // Make sure the linker puts this at the start of the kernel image

.global _start  // Execution starts here
.global secondary_start

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

    // With the Raspberry Pi armstub, only CPU0 enters the kernel image through
    // _start. Keep this guard anyway so an unexpected secondary cannot perform
    // CPU0's global runtime initialization.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbz     x1, .Lprimary_cpu
    b       park

.Lprimary_cpu:
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

    bl      el1_drop
    b       park

// Entry address written by CPU0 into the BCM2711 armstub spin table.
// The armstub parks secondary CPUs until their per-CPU release slot contains a
// non-zero physical address, then branches here with the secondary still in EL2.
secondary_start:
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff

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
