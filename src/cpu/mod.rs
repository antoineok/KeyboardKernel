use core::arch::global_asm;

global_asm!(include_str!("boot.s"));

pub mod time;