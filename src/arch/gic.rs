//! Minimal BCM2711 GIC-400 (GICv2) platform support.
//!
//! The BCM2711 device tree describes the GIC registers on the ARM-local bus.
//! `BootInfo` translates those resources through the parent `ranges` entries,
//! so after the identity MMU is enabled their ARM physical addresses can be
//! used directly as Device-nGnRE virtual addresses.

use core::{
    cell::UnsafeCell,
    ptr::{read_volatile, write_volatile},
};

use crate::{
    fdt::{AliasEntry, DeviceEntry},
    info,
};

pub const GIC_COMPATIBLE: &str = "arm,gic-400";

const GIC_SPURIOUS_MIN: u32 = 1020;
const GICC_IAR_INT_ID_MASK: u32 = 0x3ff;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicIrqAck {
    raw: u32,
    pub intid: u32,
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

    /// Initialize the banked GICv2 CPU interface for the currently executing
    /// CPU and return its Distributor target-mask bit.
    pub fn initialize_current_cpu_interface(&self) -> u8 {
        GicRegisters::from_resources(self.resources()).initialize_current_cpu_interface()
    }

    /// Configure one level-sensitive SPI and route it to a single GIC CPU
    /// interface target mask.
    pub fn configure_level_spi(&self, intid: u32, target_mask: u8, priority: u8) {
        GicRegisters::from_resources(self.resources()).configure_level_spi(
            intid,
            target_mask,
            priority,
        );
    }

    /// Software-pend an interrupt. Used only for bring-up/self-test before the
    /// real HEVC decoder starts producing completion interrupts.
    pub fn set_pending(&self, intid: u32) {
        GicRegisters::from_resources(self.resources()).set_pending(intid);
    }

    /// Acknowledge the highest-priority pending interrupt on the current CPU.
    pub fn acknowledge(&self) -> Option<GicIrqAck> {
        GicRegisters::from_resources(self.resources()).acknowledge()
    }

    /// Signal end-of-interrupt for a previously acknowledged interrupt.
    pub fn end_interrupt(&self, ack: GicIrqAck) {
        GicRegisters::from_resources(self.resources()).end_interrupt(ack);
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
        assert!(
            size <= usize::MAX as u64,
            "GIC MMIO size does not fit in usize"
        );

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

    fn write32(self, offset: usize, value: u32) {
        assert_eq!(offset & 0x3, 0, "GIC register offset is not 32-bit aligned");
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("GIC register offset overflow");
        assert!(end <= self.size, "GIC register write exceeds MMIO resource");

        unsafe {
            write_volatile((self.base + offset) as *mut u32, value);
            core::arch::asm!("dsb oshst", options(nostack, preserves_flags));
        }
    }

    fn update_byte(self, offset: usize, value: u8) {
        let word_offset = offset & !0x3;
        let shift = (offset & 0x3) * 8;
        let mut word = self.read32(word_offset);
        word &= !(0xffu32 << shift);
        word |= (value as u32) << shift;
        self.write32(word_offset, word);
    }
}

const GICD_CTLR: usize = 0x0000;
const GICD_TYPER: usize = 0x0004;
const GICD_IIDR: usize = 0x0008;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_ISPENDR: usize = 0x0200;
const GICD_ICPENDR: usize = 0x0280;
const GICD_ICACTIVER: usize = 0x0380;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ITARGETSR: usize = 0x0800;
const GICD_ICFGR: usize = 0x0c00;

const GICC_CTLR: usize = 0x0000;
const GICC_PMR: usize = 0x0004;
const GICC_IAR: usize = 0x000c;
const GICC_EOIR: usize = 0x0010;
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

    fn interrupt_lines(self) -> u32 {
        ((self.distributor.read32(GICD_TYPER) & 0x1f) + 1) * 32
    }

    fn validate_intid(self, intid: u32) {
        assert!(
            intid < self.interrupt_lines(),
            "GIC INTID {intid} is not implemented"
        );
    }

    fn current_cpu_target_mask(self) -> u8 {
        // GICD_ITARGETSR0..7 are read-only and banked for SGIs/PPIs. Linux uses
        // these registers to discover the implementation-specific CPU target
        // bit associated with the CPU interface executing the read.
        for offset in (0usize..32).step_by(4) {
            let mut mask = self.distributor.read32(GICD_ITARGETSR + offset);
            mask |= mask >> 16;
            mask |= mask >> 8;
            let mask = (mask & 0xff) as u8;
            if mask != 0 {
                assert_eq!(
                    mask.count_ones(),
                    1,
                    "GIC current CPU target mask is not one-hot"
                );
                return mask;
            }
        }

        panic!("GIC failed to expose the current CPU target mask");
    }

    fn initialize_current_cpu_interface(self) -> u8 {
        let target_mask = self.current_cpu_target_mask();

        // Preserve firmware-configured bypass/security mode bits, while making
        // sure Group 0 delivery is enabled. Use the least restrictive priority
        // mask for our own explicitly configured interrupt priorities.
        self.cpu_interface.write32(GICC_PMR, 0xff);
        let control = self.cpu_interface.read32(GICC_CTLR) | 1;
        self.cpu_interface.write32(GICC_CTLR, control);

        unsafe {
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }

        target_mask
    }

    fn configure_level_spi(self, intid: u32, target_mask: u8, priority: u8) {
        assert!(intid >= 32, "GIC INTID {intid} is not an SPI");
        self.validate_intid(intid);
        assert_ne!(target_mask, 0, "GIC SPI target mask must not be zero");

        let word = (intid / 32) as usize * 4;
        let bit = 1u32 << (intid % 32);

        // Disable while changing affinity/priority/configuration and discard any
        // stale software/hardware state before enabling the line.
        self.distributor.write32(GICD_ICENABLER + word, bit);
        self.distributor.write32(GICD_ICPENDR + word, bit);
        self.distributor.write32(GICD_ICACTIVER + word, bit);

        self.distributor
            .update_byte(GICD_IPRIORITYR + intid as usize, priority);
        self.distributor
            .update_byte(GICD_ITARGETSR + intid as usize, target_mask);

        // GICD_ICFGR allocates two bits per interrupt. For SPIs bit [1] of the
        // field selects edge (1) vs level (0); DT flags 0x4 require level-high.
        let config_offset = GICD_ICFGR + ((intid / 16) as usize * 4);
        let config_shift = (intid % 16) * 2;
        let mut config = self.distributor.read32(config_offset);
        config &= !(0b11u32 << config_shift);
        self.distributor.write32(config_offset, config);

        self.distributor.write32(GICD_ISENABLER + word, bit);
    }

    fn set_pending(self, intid: u32) {
        self.validate_intid(intid);
        let word = (intid / 32) as usize * 4;
        let bit = 1u32 << (intid % 32);
        self.distributor.write32(GICD_ISPENDR + word, bit);
    }

    fn acknowledge(self) -> Option<GicIrqAck> {
        let raw = self.cpu_interface.read32(GICC_IAR);
        let intid = raw & GICC_IAR_INT_ID_MASK;
        if intid >= GIC_SPURIOUS_MIN {
            return None;
        }

        Some(GicIrqAck { raw, intid })
    }

    fn end_interrupt(self, ack: GicIrqAck) {
        self.cpu_interface.write32(GICC_EOIR, ack.raw);
        unsafe {
            core::arch::asm!("isb", options(nostack, preserves_flags));
        }
    }
}
