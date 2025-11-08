SECTIONS
{
    . = 0x80000;     /* Kernel load address for AArch64 */
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
    _end = .;

    /* Reserve 16 KiB for the el2 stack */
    . = ALIGN(16);
    __el2_stack_start = .;
    . += 16K;
    __el2_stack_top = .;

    /* Reserve 64 KiB for the el1 stack */
    . = ALIGN(16);
    __el1_stack_start = .;
    . += 64K;
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