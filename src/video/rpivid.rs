//! Low-level register access for the BCM2711 HEVC decoder (RPIVID).
//!
//! The MMU identity-maps the BCM2711 peripheral window as Device-nGnRE, so the
//! physical addresses discovered from the FDT can be used directly as virtual
//! addresses after EL1 translation is enabled.

use core::ptr::read_volatile;

use crate::info;

use super::hevc::HevcResources;

// Phase-1 HEVC register offsets used during initial hardware bring-up.
const RPI_CONTROL: usize = 0x0034;
const RPI_STATUS: usize = 0x0038;
const RPI_VERSION: usize = 0x003c;

// Phase-2 status register. The phase-2 register file begins at 0x8000.
const RPI_STATUS2: usize = 0x8028;

// Local Argon interrupt controller register block.
const ARG_IC_ICTRL: usize = 0x0000;

const BCM2711_HEVC_VERSION: u32 = 0x0202;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MmioRegion {
    base: usize,
    size: usize,
}

impl MmioRegion {
    fn new(physical_base: u64, size: u64) -> Self {
        assert!(
            physical_base <= usize::MAX as u64,
            "HEVC MMIO base does not fit in usize"
        );
        assert!(size <= usize::MAX as u64, "HEVC MMIO size does not fit in usize");

        Self {
            base: physical_base as usize,
            size: size as usize,
        }
    }

    fn read32(self, offset: usize) -> u32 {
        assert_eq!(offset & 0x3, 0, "HEVC register offset is not 32-bit aligned");
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("HEVC register offset overflow");
        assert!(end <= self.size, "HEVC register read exceeds MMIO resource");

        unsafe { read_volatile((self.base + offset) as *const u32) }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividIdleSnapshot {
    pub version: u32,
    pub control: u32,
    pub status1: u32,
    pub status2: u32,
    pub interrupt_control: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividRegisters {
    hevc: MmioRegion,
    intc: MmioRegion,
}

impl RpividRegisters {
    pub fn from_resources(resources: HevcResources) -> Self {
        Self {
            hevc: MmioRegion::new(resources.hevc.arm_phys, resources.hevc.size),
            intc: MmioRegion::new(resources.intc.arm_phys, resources.intc.size),
        }
    }

    pub fn idle_snapshot(self) -> RpividIdleSnapshot {
        RpividIdleSnapshot {
            version: self.hevc.read32(RPI_VERSION),
            control: self.hevc.read32(RPI_CONTROL),
            status1: self.hevc.read32(RPI_STATUS),
            status2: self.hevc.read32(RPI_STATUS2),
            interrupt_control: self.intc.read32(ARG_IC_ICTRL),
        }
    }

    pub fn probe(self) -> RpividIdleSnapshot {
        let snapshot = self.idle_snapshot();
        assert_eq!(
            snapshot.version, BCM2711_HEVC_VERSION,
            "unsupported BCM2711 HEVC hardware version {:#x}",
            snapshot.version
        );

        info!(
            "BCM2711 HEVC register probe passed: version={:#x}, control={:#010x}, status1={:#010x}, status2={:#010x}, intc={:#010x}, hevc={:#018x}, intc_base={:#018x}",
            snapshot.version,
            snapshot.control,
            snapshot.status1,
            snapshot.status2,
            snapshot.interrupt_control,
            self.hevc.base,
            self.intc.base,
        );

        snapshot
    }
}
