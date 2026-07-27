use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicU8, Ordering},
};

use super::{current_id, send_event, wait_for_event, CpuId, CPU_COUNT};
use crate::arch::{exceptions::PrivilegeLevel, mmu};

// The Raspberry Pi armstub keeps secondary BCM2711 CPUs in a spin table in
// physical page zero. CPU1/2/3 release slots are 0xE0, 0xE8 and 0xF0.
const CPU_RELEASE_ADDRS: [usize; CPU_COUNT] = [0xD8, 0xE0, 0xE8, 0xF0];

const STATUS_ONLINE: u8 = 1 << 0;
const STATUS_EL1: u8 = 1 << 1;
const STATUS_MMU: u8 = 1 << 2;
const STATUS_DCACHE: u8 = 1 << 3;

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
    pub data_cache_enabled: bool,
}

impl SecondaryCpuStatus {
    const OFFLINE: Self = Self {
        online: false,
        at_el1: false,
        mmu_enabled: false,
        data_cache_enabled: false,
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
        // The spin-table alias is Normal-NC, so the armstub observes the release
        // slot directly even after CPU0 enables its data cache.
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
        data_cache_enabled: status & STATUS_DCACHE != 0,
    }
}

fn publish_secondary_status(cpu: CpuId, at_el1: bool, mmu_enabled: bool, data_cache_enabled: bool) {
    let mut status = STATUS_ONLINE;

    if at_el1 {
        status |= STATUS_EL1;
    }

    if mmu_enabled {
        status |= STATUS_MMU;
    }

    if data_cache_enabled {
        status |= STATUS_DCACHE;
    }

    SECONDARY_STATUS[cpu.index()].store(status, Ordering::Release);

    send_event();
}

/// First Rust entry point for secondary CPUs.
///
/// Every secondary first installs CPU0's published translation tables and
/// reports its EL1/MMU status. CPU1 then transfers into the dedicated graphics
/// worker and CPU2 into the dedicated USB transport worker; any other released
/// secondary CPU remains parked.
#[no_mangle]
pub extern "C" fn secondary_el1_entry() -> ! {
    let cpu = current_id();

    let mmu_installed = unsafe { mmu::enable_published_kernel_mmu() };
    let at_el1 = PrivilegeLevel::current().is_kernel();
    let mmu_enabled = mmu_installed && mmu::is_enabled();
    let data_cache_enabled = mmu_installed && mmu::is_data_cache_enabled();

    if cpu.raw() == 1 {
        crate::video::hevc::HEVC_PLATFORM.initialize_irq_on_current_cpu();
    }

    publish_secondary_status(cpu, at_el1, mmu_enabled, data_cache_enabled);

    match cpu.raw() {
        1 => crate::graphics::run_graphics_core(),
        2 => crate::transport::run_usb_transport_core(),
        _ => {}
    }

    loop {
        wait_for_event();
    }
}
