//! Minimal BCM2711 GIC-400 (GICv2) platform discovery and register probe.
//!
//! The BCM2711 device tree describes the GIC registers on the ARM-local bus.
//! `BootInfo` translates those resources through the parent `ranges` entries,
//! so after the identity MMU is enabled their ARM physical addresses can be
//! used directly as Device-nGnRE virtual addresses.

use core::{cell::UnsafeCell, ptr::read_volatile};

use crate::{fdt::{AliasEntry, DeviceEntry}, info};

pub const GIC_COMPATIBLE: &str = "arm,gic-400";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicResources {
    pub distributor: AliasEntry,
    pub cpu_interface: AliasEntry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicProbe {
    pub distributor_control: u32,
    pub distributor_typer: u32,
    pub distributor_iidr: u32,
    pub cpu_control: u32,
    pub cpu_priority_mask: u32,
    pub cpu_iidr: u32,
    pub interrupt_lines: u32,
    pub cpu_interfaces: u32,
}

pub struct GicPlatform(UnsafeCell<Option<GicResources>>);

unsafe impl Sync for GicPlatform {}

pub static GIC_PLATFORM: GicPlatform = GicPlatform(UnsafeCell::new(None));

impl GicPlatform {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    pub fn init(&self, device: Option<DeviceEntry>) {
        let device = device.unwrap_or_else(|| panic!("BCM2711 GIC-400 not found in device tree"));
        let resources = device.resources();

        // The arm,gic binding defines the first register region as the
        // Distributor and the second as the CPU interface. BCM2711 also
        // exposes the virtualization control/CPU regions after those two.
        assert!(
            resources.len() >= 2,
            "BCM2711 GIC-400 must expose Distributor and CPU-interface resources"
        );

        let distributor = resources[0].entry;
        let cpu_interface = resources[1].entry;

        assert_eq!(
            distributor.arm_phys & 0xfff,
            0,
            "GIC Distributor base is not page aligned"
        );
        assert_eq!(
            cpu_interface.arm_phys & 0xfff,
            0,
            "GIC CPU-interface base is not page aligned"
        );

        unsafe {
            let slot = &mut *self.0.get();
            assert!(slot.is_none(), "GIC platform resources already initialized");
            *slot = Some(GicResources {
                distributor,
                cpu_interface,
            });
        }

        info!(
            "GIC-400 discovered: distributor bus={:#018x} phys={:#018x} size={} KiB, cpu bus={:#018x} phys={:#018x} size={} KiB",
            distributor.vc_addr,
            distributor.arm_phys,
            distributor.size / 1024,
            cpu_interface.vc_addr,
            cpu_interface.arm_phys,
            cpu_interface.size / 1024,
        );
    }

    pub fn probe(&self) -> GicProbe {
        GicRegisters::from_resources(self.resources()).probe()
    }

    pub fn resources(&self) -> GicResources {
        unsafe { (*self.0.get()).expect("GIC resources requested before initialization") }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MmioRegion {
    base: usize,
    size: usize,
}

impl MmioRegion {
    fn new(physical_base: u64, size: u64) -> Self {
        assert!(
            physical_base <= usize::MAX as u64,
            "GIC MMIO base does not fit in usize"
        );
        assert!(size <= usize::MAX as u64, "GIC MMIO size does not fit in usize");

        Self {
            base: physical_base as usize,
            size: size as usize,
        }
    }

    fn read32(self, offset: usize) -> u32 {
        assert_eq!(offset & 0x3, 0, "GIC register offset is not 32-bit aligned");
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("GIC register offset overflow");
        assert!(end <= self.size, "GIC register read exceeds MMIO resource");

        unsafe { read_volatile((self.base + offset) as *const u32) }
    }
}

const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IIDR: usize = 0x0008;

const GICC_CTLR: usize = 0x0000;
const GICC_PMR: usize = 0x0004;
const GICC_IIDR: usize = 0x00fc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GicRegisters {
    distributor: MmioRegion,
    cpu_interface: MmioRegion,
}

impl GicRegisters {
    fn from_resources(resources: GicResources) -> Self {
        Self {
            distributor: MmioRegion::new(
                resources.distributor.arm_phys,
                resources.distributor.size,
            ),
            cpu_interface: MmioRegion::new(
                resources.cpu_interface.arm_phys,
                resources.cpu_interface.size,
            ),
        }
    }

    fn probe(self) -> GicProbe {
        let distributor_typer = self.distributor.read32(GICD_TYPER);
        let interrupt_lines = ((distributor_typer & 0x1f) + 1) * 32;
        let cpu_interfaces = ((distributor_typer >> 5) & 0x7) + 1;

        let snapshot = GicProbe {
            distributor_control: self.distributor.read32(GICD_CTLR),
            distributor_typer,
            distributor_iidr: self.distributor.read32(GICD_IIDR),
            cpu_control: self.cpu_interface.read32(GICC_CTLR),
            cpu_priority_mask: self.cpu_interface.read32(GICC_PMR),
            cpu_iidr: self.cpu_interface.read32(GICC_IIDR),
            interrupt_lines,
            cpu_interfaces,
        };

        info!(
            "GIC-400 register probe passed: dist_ctlr={:#010x}, typer={:#010x}, dist_iidr={:#010x}, cpu_ctlr={:#010x}, pmr={:#010x}, cpu_iidr={:#010x}, interrupt_lines={}, cpu_interfaces={}",
            snapshot.distributor_control,
            snapshot.distributor_typer,
            snapshot.distributor_iidr,
            snapshot.cpu_control,
            snapshot.cpu_priority_mask,
            snapshot.cpu_iidr,
            snapshot.interrupt_lines,
            snapshot.cpu_interfaces,
        );

        snapshot
    }
}
