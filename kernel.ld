SECTIONS
{
    . = 0x80000;     /* Kernel load address for AArch64 */
    __kernel_start = .;
    .text : { KEEP(*(.text.boot)) *(.text .text.* .gnu.linkonce.t*) }
    .rodata : { *(.rodata .rodata.* .gnu.linkonce.r*) }
    PROVIDE(_data = .);
    .data : { *(.data .data.* .gnu.linkonce.d*) }
    .bss (NOLOAD) : {
        . = ALIGN(16);
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }
    .update_magic : {
        KEEP(*(.update_magic))
    }
    __kernel_end = .;

    /* Reserve a private 16 KiB EL2 transition stack for every CPU. */
    . = ALIGN(16);
    __el2_stack_start = .;

    __el2_stack_cpu0_start = .;
    . += 16K;
    __el2_stack_cpu0_top = .;

    __el2_stack_cpu1_start = .;
    . += 16K;
    __el2_stack_cpu1_top = .;

    __el2_stack_cpu2_start = .;
    . += 16K;
    __el2_stack_cpu2_top = .;

    __el2_stack_cpu3_start = .;
    . += 16K;
    __el2_stack_cpu3_top = .;

    __el2_stack_top = .;

    /* Reserve a private 64 KiB EL1 runtime stack for every CPU. */
    . = ALIGN(16);
    __el1_stack_start = .;

    __el1_stack_cpu0_start = .;
    . += 64K;
    __el1_stack_cpu0_top = .;

    __el1_stack_cpu1_start = .;
    . += 64K;
    __el1_stack_cpu1_top = .;

    __el1_stack_cpu2_start = .;
    . += 64K;
    __el1_stack_cpu2_top = .;

    __el1_stack_cpu3_start = .;
    . += 64K;
    __el1_stack_cpu3_top = .;

    __el1_stack_top = .;

    /* Align to 4 KiB (page boundary for safety) */
    . = ALIGN(0x1000);
    __heap_start = .;

    /* Reserve 512 KiB for heap (tweak as you wish) */
    . = . + 512K;
    __heap_end = .;

   /DISCARD/ : { *(.comment) *(.gnu*) *(.note*) *(.eh_frame*) }
}
__bss_size = (__bss_end - __bss_start)>>3;
__kernel_size = __kernel_end - __kernel_start;