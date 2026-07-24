use core::{
    arch::asm,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{current_id, CpuId};
use crate::arch::{exceptions::PrivilegeLevel, mmu};

// Keep this non-zero so the release word is emitted into `.data`, not `.bss`.
// Secondary CPUs are already spinning before CPU0 clears `.bss`, so a zero-
// initialized release word could otherwise contain arbitrary RAM contents for
// a short period and spuriously release a CPU during early boot.
const RELEASE_WORD_SENTINEL: u64 = 1 << 63;

/// Bitmask observed directly by the assembly secondary-CPU parking loop.
///
/// Low bits 0..CPU_COUNT are release requests. Bit 63 is a permanent sentinel
/// used only to keep this word out of `.bss` before CPU0 has cleared it.
#[no_mangle]
pub static SECONDARY_RELEASE_MASK: AtomicU64 = AtomicU64::new(RELEASE_WORD_SENTINEL);

static SECONDARY_ONLINE_MASK: AtomicU64 = AtomicU64::new(0);
static SECONDARY_EL1_MASK: AtomicU64 = AtomicU64::new(0);
static SECONDARY_MMU_MASK: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecondaryCpuStatus {
    pub online: bool,
    pub at_el1: bool,
    pub mmu_enabled: bool,
}

impl SecondaryCpuStatus {
    const OFFLINE: Self = Self {
        online: false,
        at_el1: false,
        mmu_enabled: false,
    };
}

#[inline(always)]
const fn cpu_bit(cpu: CpuId) -> u64 {
    1u64 << cpu.index()
}

/// Release one already-parked secondary CPU from the assembly wait loop.
///
/// The boot CPU is deliberately excluded: it is already executing the kernel.
pub fn release_secondary(cpu: CpuId) -> bool {
    if cpu.is_boot() {
        return false;
    }

    // CPU0 is the sole writer of the release word. Avoid an atomic RMW during
    // initial SMP bring-up; a simple release store is sufficient and avoids an
    // LDXR/STXR retry loop before secondary-core coherency is fully validated.
    crate::panic::panic_logger::early_aux_write_str(
        "[SMP] release_secondary: publishing release bit\n",
    );

    let release_mask = SECONDARY_RELEASE_MASK.load(Ordering::Relaxed);
    SECONDARY_RELEASE_MASK.store(release_mask | cpu_bit(cpu), Ordering::Release);

    crate::panic::panic_logger::early_aux_write_str(
        "[SMP] release_secondary: release bit stored\n",
    );

    // CPU1 is still executing in EL2 with its MMU disabled. Use a full-system
    // barrier for this first bring-up rather than depending on stage-1
    // shareability attributes that only exist on CPU0 at this point.
    unsafe {
        asm!("dsb sy", "sev", options(nostack, preserves_flags));
    }

    crate::panic::panic_logger::early_aux_write_str(
        "[SMP] release_secondary: SEV sent\n",
    );

    true
}

#[inline(always)]
pub fn secondary_status(cpu: CpuId) -> SecondaryCpuStatus {
    if cpu.is_boot() {
        return SecondaryCpuStatus::OFFLINE;
    }

    let bit = cpu_bit(cpu);

    SecondaryCpuStatus {
        online: SECONDARY_ONLINE_MASK.load(Ordering::Acquire) & bit != 0,
        at_el1: SECONDARY_EL1_MASK.load(Ordering::Acquire) & bit != 0,
        mmu_enabled: SECONDARY_MMU_MASK.load(Ordering::Acquire) & bit != 0,
    }
}

fn publish_secondary_status(cpu: CpuId, at_el1: bool, mmu_enabled: bool) {
    let bit = cpu_bit(cpu);

    if at_el1 {
        SECONDARY_EL1_MASK.fetch_or(bit, Ordering::Relaxed);
    }

    if mmu_enabled {
        SECONDARY_MMU_MASK.fetch_or(bit, Ordering::Relaxed);
    }

    // Publish `online` last. CPU0's Acquire load of this bit synchronizes with
    // all status writes above.
    SECONDARY_ONLINE_MASK.fetch_or(bit, Ordering::Release);

    unsafe {
        asm!("sev", options(nomem, nostack, preserves_flags));
    }
}

/// First Rust entry point for secondary CPUs.
///
/// This deliberately touches no existing mutable kernel singleton. Until the
/// SMP ownership/synchronization audit is complete, a secondary CPU may only:
/// - identify itself,
/// - install the already-built CPU0 translation tables locally,
/// - publish status through atomics,
/// - park again.
#[no_mangle]
pub extern "C" fn secondary_el1_entry() -> ! {
    let cpu = current_id();

    let mmu_installed = unsafe { mmu::enable_published_kernel_mmu() };
    let at_el1 = PrivilegeLevel::current().is_kernel();
    let mmu_enabled = mmu_installed && mmu::is_enabled();

    publish_secondary_status(cpu, at_el1, mmu_enabled);

    loop {
        unsafe {
            asm!("wfe", options(nomem, nostack, preserves_flags));
        }
    }
}
