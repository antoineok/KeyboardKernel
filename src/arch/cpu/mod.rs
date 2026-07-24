use core::arch::global_asm;

mod id;

pub use id::{current_id, CpuId, CPU_COUNT};

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
