use core::cell::UnsafeCell;

use crate::{fdt::{AliasEntry, DeviceEntry}, info};

pub const HEVC_COMPATIBLE: &str = "brcm,bcm2711-hevc-dec";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HevcResources {
    pub hevc: AliasEntry,
    pub intc: AliasEntry,
}

pub struct HevcPlatform(UnsafeCell<Option<HevcResources>>);

unsafe impl Sync for HevcPlatform {}

pub static HEVC_PLATFORM: HevcPlatform = HevcPlatform(UnsafeCell::new(None));

impl HevcPlatform {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    pub fn init(&self, device: Option<DeviceEntry>) {
        let device = device.unwrap_or_else(|| {
            panic!("BCM2711 HEVC decoder not found in device tree")
        });

        let hevc = device
            .resource("hevc")
            .unwrap_or_else(|| panic!("BCM2711 HEVC decoder has no 'hevc' register resource"));
        let intc = device
            .resource("intc")
            .unwrap_or_else(|| panic!("BCM2711 HEVC decoder has no 'intc' register resource"));

        assert_eq!(hevc.arm_phys & 0xfff, 0, "HEVC register base is not page aligned");
        assert_eq!(intc.arm_phys & 0xfff, 0, "HEVC interrupt-controller base is not page aligned");

        unsafe {
            let slot = &mut *self.0.get();
            assert!(slot.is_none(), "HEVC platform resources already initialized");
            *slot = Some(HevcResources { hevc, intc });
        }

        info!(
            "HEVC decoder discovered: hevc bus={:#018x} phys={:#018x} size={} KiB, intc bus={:#018x} phys={:#018x} size={} KiB",
            hevc.vc_addr,
            hevc.arm_phys,
            hevc.size / 1024,
            intc.vc_addr,
            intc.arm_phys,
            intc.size / 1024,
        );
    }

    pub fn resources(&self) -> HevcResources {
        unsafe {
            (*self.0.get())
                .expect("HEVC platform resources requested before initialization")
        }
    }
}
