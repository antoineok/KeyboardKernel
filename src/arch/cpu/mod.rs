use core::arch::{asm, global_asm};

mod id;
mod smp;
mod spsc;

pub use id::{current_id, CpuId, CPU_COUNT};
pub use smp::{release_secondary, secondary_status, SecondaryCpuStatus};
pub use spsc::SpscQueue;

/// Publish prior shared-memory stores before waking CPUs blocked in `WFE`.
///
/// `DSB ISHST` is intentionally paired with `SEV`: the event may be observed
/// immediately by another core, so shared queue/state stores must already be
/// visible in the inner-shareable domain before the wakeup is sent.  Do not
/// mark this asm `nomem`; Rust otherwise assumes it cannot synchronize with
/// other threads and may reorder memory operations across it.
#[inline(always)]
pub fn send_event() {
    unsafe {
        asm!("dsb ishst", "sev", options(nostack, preserves_flags),);
    }
}

/// Wait for an event from another CPU.
///
/// This is a compiler synchronization point as well as a CPU sleep hint; the
/// following acquire loads must not be hoisted across the wait.
#[inline(always)]
pub fn wait_for_event() {
    unsafe {
        asm!("wfe", options(nostack, preserves_flags));
    }
}

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
