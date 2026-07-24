    .text
    .align  4
    .global el1_drop

el1_drop:

    // --- Prepare this CPU's private EL1 stack and vectors ---
    // Keep x0 untouched: on CPU0 it carries the firmware DTB pointer into Rust.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff

    cmp     x1, #0
    b.eq    .Lel1_stack_cpu0
    cmp     x1, #1
    b.eq    .Lel1_stack_cpu1
    cmp     x1, #2
    b.eq    .Lel1_stack_cpu2
    cmp     x1, #3
    b.eq    .Lel1_stack_cpu3
    b       park

.Lel1_stack_cpu0:
    ldr     x1, =__el1_stack_cpu0_top
    b       .Lel1_stack_ready
.Lel1_stack_cpu1:
    ldr     x1, =__el1_stack_cpu1_top
    b       .Lel1_stack_ready
.Lel1_stack_cpu2:
    ldr     x1, =__el1_stack_cpu2_top
    b       .Lel1_stack_ready
.Lel1_stack_cpu3:
    ldr     x1, =__el1_stack_cpu3_top

.Lel1_stack_ready:
    msr     sp_el1, x1

    ldr     x1, =el1_exception_vector
    msr     vbar_el1, x1
    isb

    // --- Timers: allow EL1 access to physical counter/timer ---
    mrs     x2, cnthctl_el2
    orr     x2, x2, #0x3              // EL1PCTEN | EL1PCEN
    msr     cnthctl_el2, x2
    msr     cntvoff_el2, xzr

    // --- Optional: populate virtual ID registers ---
    mrs     x2, midr_el1
    mrs     x3, mpidr_el1
    msr     vpidr_el2, x2
    msr     vmpidr_el2, x3

    // --- Disable traps to EL2, enable FP/SIMD at EL1 ---
    mov     x2, xzr
    msr     cptr_el2, x2
    msr     hstr_el2, xzr
    mov     x2, #(3 << 20)            // CPACR_EL1.FPEN=11
    msr     cpacr_el1, x2

    // --- HCR_EL2: AArch64 EL1, no VHE, no traps ---
    mov     x2, #(1 << 31)            // RW=1 (EL1 AArch64)
    msr     hcr_el2, x2

    // --- SCTLR_EL1 baseline: RES1 bits set, MMU/caches OFF ---
    mov     x2, #0x0800
    movk    x2, #0x30d0, lsl #16
    msr     sctlr_el1, x2
    isb

    // --- Program return state: EL1h, interrupts masked (DAIF) ---
    mov     x2, #(0x3c0 | 0x5)
    msr     spsr_el2, x2

    // --- Select the EL1 Rust entry point for this CPU ---
    // CPU0 performs global kernel initialization. Secondary CPUs enter a
    // minimal SMP bring-up path that installs the shared translation tables,
    // publishes their status through atomics, and parks again.
    mrs     x2, mpidr_el1
    and     x2, x2, #0xff
    cbz     x2, .Lel1_primary_entry

    ldr     x2, =secondary_el1_entry
    b       .Lel1_entry_ready

.Lel1_primary_entry:
    ldr     x2, =el1_entry

.Lel1_entry_ready:
    msr     elr_el2, x2

    // --- Synchronize and drop ---
    isb
    eret

.global park
park:
    wfi
    b park