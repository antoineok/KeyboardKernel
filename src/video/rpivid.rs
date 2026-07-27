//! Low-level register access for the BCM2711 HEVC decoder (RPIVID).
//!
//! The MMU identity-maps the BCM2711 peripheral window as Device-nGnRE, so the
//! physical addresses discovered from the FDT can be used directly as virtual
//! addresses after EL1 translation is enabled.

use core::ptr::read_volatile;

use crate::info;

use super::hevc::HevcResources;

/// Phase-1 HEVC register containing the decoder hardware revision.
///
/// This matches `RPI_VERSION` in Raspberry Pi's HEVC decoder driver.
const RPI_VERSION: usize = 0x003c;
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

    pub fn version(self) -> u32 {
        self.hevc.read32(RPI_VERSION)
    }

    pub fn probe(self) -> u32 {
        let version = self.version();
        assert_eq!(
            version, BCM2711_HEVC_VERSION,
            "unsupported BCM2711 HEVC hardware version {version:#x}"
        );

        info!(
            "BCM2711 HEVC register probe passed: version={:#x}, hevc={:#018x}, intc={:#018x}",
            version, self.hevc.base, self.intc.base
        );

        version
    }
}
