use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicU8, Ordering},
};

use super::{current_id, CpuId, CPU_COUNT};
use crate::arch::{exceptions::PrivilegeLevel, mmu};

// The Raspberry Pi armstub keeps secondary BCM2711 CPUs in a spin table in
// physical page zero. CPU1/2/3 release slots are 0xE0, 0xE8 and 0xF0.
const CPU_RELEASE_ADDRS: [usize; CPU_COUNT] = [0xD8, 0xE0, 0xE8, 0xF0];

const STATUS_ONLINE: u8 = 1 << 0;
const STATUS_EL1: u8 = 1 << 1;
const STATUS_MMU: u8 = 1 << 2;

static SECONDARY_STATUS: [AtomicU8; CPU_COUNT] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

extern "C" {
    fn secondary_start();
}

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

/// Release one firmware-parked BCM2711 secondary CPU through the armstub spin
/// table. The boot CPU is deliberately excluded: it is already executing.
pub fn release_secondary(cpu: CpuId) -> bool {
    if cpu.is_boot() {
        return false;
    }

    let release_offset = CPU_RELEASE_ADDRS[cpu.index()];
    let release_slot = ((mmu::SECONDARY_SPIN_TABLE_VIRT as usize) + release_offset) as *mut usize;
    let entry_phys = secondary_start as usize;

    unsafe {
        // The spin-table alias is Normal-NC and D-cache is still disabled, so
        // the armstub can observe the physical slot directly.
        ptr::write_volatile(release_slot, entry_phys);

        // Complete the release-slot write before waking the firmware's WFE
        // loop. The spin-table protocol uses SEV as the wake-up event.
        asm!("dsb sy", "sev", options(nostack, preserves_flags));
    }

    true
}

#[inline(always)]
pub fn secondary_status(cpu: CpuId) -> SecondaryCpuStatus {
    if cpu.is_boot() {
        return SecondaryCpuStatus::OFFLINE;
    }

    let status = SECONDARY_STATUS[cpu.index()].load(Ordering::Acquire);

    SecondaryCpuStatus {
        online: status & STATUS_ONLINE != 0,
        at_el1: status & STATUS_EL1 != 0,
        mmu_enabled: status & STATUS_MMU != 0,
    }
}

fn publish_secondary_status(cpu: CpuId, at_el1: bool, mmu_enabled: bool) {
    let mut status = STATUS_ONLINE;

    if at_el1 {
        status |= STATUS_EL1;
    }

    if mmu_enabled {
        status |= STATUS_MMU;
    }

    SECONDARY_STATUS[cpu.index()].store(status, Ordering::Release);

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
