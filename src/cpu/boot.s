.global _start

.section ".text.boot"

_start:
    // On entry, x0 = FDT pointer (don't clobber!)

    // Only proceed if the core executes in EL2. Park it otherwise.
    mrs	x15, CurrentEL
    cmp	x15, #0x8           // EL2 encoded as 0b1000
    b.ne	.L_parking_loop

    // Check processor ID is zero (executing on main core), else hang
    mrs     x1, mpidr_el1
    and     x1, x1, #3
    cbz     x1, 2f
1:  wfe
    b       1b
2:
    // Save the DTB pointer from x0 into x20 (or pass down to Rust later)
    mov     x20, x0

    // Set stack to start below our code
    ldr     x1, =__stack_top
    mov     sp, x1

    // Clean the BSS
    ldr     x1, =__bss_start
    ldr     w2, =__bss_size
3:  cbz     w2, 4f
    str     xzr, [x1], #8
    sub     w2, w2, #1
    cbnz    w2, 3b

    // Read CNTFRQ_EL0
    adrp	x1, ARCH_TIMER_COUNTER_FREQUENCY
	add	x1, x1, #:lo12:ARCH_TIMER_COUNTER_FREQUENCY
	mrs	x2, CNTFRQ_EL0
	cmp	x2, xzr
	b.eq	.L_parking_loop
	str	w2, [x1]

    // Jump to Rust entry, passing:
    //   x0 = stack top
    //   x1 = fdt pointer
4:  ldr     x0, =__stack_top        // your chosen stack top (maybe phys_boot_core_stack_end_exclusive_addr)
    mov     x1, x20                 // dtb pointer
    bl      _start_rust

    // shouldn't return
    b       1b

.L_parking_loop:
    wfe
    b .L_parking_loop
