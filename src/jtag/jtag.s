.global wait_for_debugger

wait_for_debugger:
    // Use a memory location as a flag
    ldr x0, =debugger_flag
1:
    ldr w1, [x0]
    cbnz w1, 2f      // If flag set, continue
    nop              // Otherwise, sleep until event
    b 1b
2:
    ret

.section .bss
.align 3
debugger_flag:
    .word 0