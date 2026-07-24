use core::arch::global_asm;

mod id;
mod smp;
mod spsc;

pub use id::{current_id, CpuId, CPU_COUNT};
pub use smp::{release_secondary, secondary_status, SecondaryCpuStatus};
pub use spsc::SpscQueue;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
