use core::arch::global_asm;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
