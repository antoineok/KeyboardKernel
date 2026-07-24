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

    // Check MPIDR_EL1.Aff0 is zero (executing on the boot CPU), else park.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbz     x1, 2f
    // We're not on the main core, so hang in an infinite wait loop
1:  wfe
    b       1b
2:  // We're on the main core!

    // Check if in EL2
    mrs     x1, CurrentEL
    and     x1, x1, 2
    cbz     x1, 3f

3:
    // CPU0 owns its EL2 transition stack. Secondary CPUs remain parked above.
    ldr     x1, =__el2_stack_cpu0_top
    mov     sp, x1

    // Clean the BSS section
    ldr     x1, =__bss_start     // Start address
    ldr     w2, =__bss_size      // Size of the section
3:  cbz     w2, 4f               // Quit loop if zero
    str     xzr, [x1], #8
    sub     w2, w2, #1
    cbnz    w2, 3b               // Loop if non-zero

    adrp	x1, ARCH_TIMER_COUNTER_FREQUENCY
	add	x1, x1, #:lo12:ARCH_TIMER_COUNTER_FREQUENCY
	mrs	x2, CNTFRQ_EL0
	cmp	x2, xzr
	b.eq	park
	str	w2, [x1]

    // Jump to our el1_drop
4:  bl      el1_drop
    // In case it does return, halt the master core too
    b       1b