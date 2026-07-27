//! Low-level register access for the BCM2711 HEVC decoder (RPIVID).
//!
//! The MMU identity-maps the BCM2711 peripheral window as Device-nGnRE, so the
//! physical addresses discovered from the FDT can be used directly as virtual
//! addresses after EL1 translation is enabled.

use core::ptr::{read_volatile, write_volatile};

use crate::{info, memory::dma::DmaBuffer};

use super::hevc::HevcResources;

// Phase-1 HEVC register offsets used during initial hardware bring-up.
const RPI_CONTROL: usize = 0x0034;
const RPI_STATUS: usize = 0x0038;
const RPI_VERSION: usize = 0x003c;

// Phase-2 status register. The phase-2 register file begins at 0x8000.
const RPI_STATUS2: usize = 0x8028;

// Local Argon interrupt controller register block.
const ARG_IC_ICTRL: usize = 0x0000;

const ARG_IC_ICTRL_ACTIVE1_INT: u32 = 1 << 0;
const ARG_IC_ICTRL_ACTIVE1_EN: u32 = 1 << 2;
const ARG_IC_ICTRL_ACTIVE2_INT: u32 = 1 << 4;
const ARG_IC_ICTRL_ACTIVE2_EN: u32 = 1 << 6;
const ARG_IC_ICTRL_H264_INTERRUPT_INT: u32 = 1 << 25;
const ARG_IC_ICTRL_VP9_INTERRUPT_INT: u32 = 1 << 29;

const ARG_IC_ICTRL_HEVC_ENABLE_MASK: u32 =
    ARG_IC_ICTRL_ACTIVE1_EN | ARG_IC_ICTRL_ACTIVE2_EN;
const ARG_IC_ICTRL_ALL_IRQ_MASK: u32 = ARG_IC_ICTRL_ACTIVE1_INT
    | ARG_IC_ICTRL_ACTIVE2_INT
    | ARG_IC_ICTRL_H264_INTERRUPT_INT
    | ARG_IC_ICTRL_VP9_INTERRUPT_INT;

const BCM2711_HEVC_VERSION: u32 = 0x0202;

const RPIVID_DMA_GRANULE: usize = 64;
const RPIVID_COMMAND_CAPACITY: usize = 8192;
const RPIVID_COMMAND_BYTES: usize = RPIVID_COMMAND_CAPACITY * core::mem::size_of::<u64>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividDmaAddress(u32);

impl RpividDmaAddress {
    pub fn from_phys(phys_addr: u64) -> Self {
        assert_eq!(
            phys_addr & (RPIVID_DMA_GRANULE as u64 - 1),
            0,
            "RPIVID DMA address is not 64-byte aligned"
        );

        let encoded = phys_addr >> 6;
        assert!(
            encoded <= u32::MAX as u64,
            "RPIVID DMA address exceeds hardware register width"
        );

        Self(encoded as u32)
    }

    #[inline(always)]
    pub const fn register_value(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividDmaLength(u32);

impl RpividDmaLength {
    pub fn from_bytes(bytes: usize) -> Self {
        let units = bytes
            .checked_add(RPIVID_DMA_GRANULE - 1)
            .expect("RPIVID DMA length overflow")
            / RPIVID_DMA_GRANULE;
        assert!(
            units <= u32::MAX as usize,
            "RPIVID DMA length exceeds hardware register width"
        );

        Self(units as u32)
    }

    #[inline(always)]
    pub const fn register_value(self) -> u32 {
        self.0
    }
}

pub struct RpividCommandBuffer {
    dma: DmaBuffer,
}

impl RpividCommandBuffer {
    pub fn allocate() -> Self {
        let dma = DmaBuffer::allocate(RPIVID_COMMAND_BYTES, RPIVID_DMA_GRANULE)
            .expect("failed to allocate RPIVID command DMA buffer");

        Self { dma }
    }

    #[inline(always)]
    pub fn phys_addr(&self) -> u64 {
        self.dma.phys_addr()
    }

    #[inline(always)]
    pub fn hardware_addr(&self) -> RpividDmaAddress {
        RpividDmaAddress::from_phys(self.phys_addr())
    }

    #[inline(always)]
    pub fn hardware_len(&self) -> RpividDmaLength {
        RpividDmaLength::from_bytes(self.dma.len())
    }

    #[inline(always)]
    pub const fn command_capacity(&self) -> usize {
        RPIVID_COMMAND_CAPACITY
    }

    pub fn sync_for_device(&self) {
        self.dma.sync_for_device();
    }
}

pub fn validate_command_dma_buffer() {
    let command = RpividCommandBuffer::allocate();

    assert_eq!(
        command.phys_addr() as usize % RPIVID_DMA_GRANULE,
        0,
        "RPIVID command buffer is not 64-byte aligned"
    );
    assert_eq!(command.command_capacity(), RPIVID_COMMAND_CAPACITY);
    assert_eq!(
        command.hardware_len().register_value(),
        (RPIVID_COMMAND_BYTES / RPIVID_DMA_GRANULE) as u32
    );

    command.sync_for_device();

    info!(
        "RPIVID command DMA buffer ready: phys={:#018x}, hw_addr={:#010x}, size={} KiB, hw_len={}x64B, commands={}",
        command.phys_addr(),
        command.hardware_addr().register_value(),
        RPIVID_COMMAND_BYTES / 1024,
        command.hardware_len().register_value(),
        command.command_capacity(),
    );
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

    fn write32(self, offset: usize, value: u32) {
        assert_eq!(offset & 0x3, 0, "HEVC register offset is not 32-bit aligned");
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("HEVC register offset overflow");
        assert!(end <= self.size, "HEVC register write exceeds MMIO resource");

        unsafe {
            write_volatile((self.base + offset) as *mut u32, value);
            // The local interrupt controller is Device memory. Complete this
            // write before we inspect the resulting latch state.
            core::arch::asm!("dsb oshst", options(nostack, preserves_flags));
        }
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
pub struct RpividInterruptInitSnapshot {
    pub before: u32,
    pub enabled: u32,
    pub after_clear: u32,
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


    /// Acknowledge any decoder-local interrupt latches.
    ///
    /// The Argon interrupt latch bits are write-one-to-clear. The enable bits
    /// share the same register, so preserve the complete register value when
    /// clearing pending sources.
    pub fn acknowledge_interrupts(self) -> u32 {
        let state = self.intc.read32(ARG_IC_ICTRL);
        let pending = state & ARG_IC_ICTRL_ALL_IRQ_MASK;

        if pending != 0 {
            self.intc.write32(ARG_IC_ICTRL, state);
        }

        pending
    }

    pub fn initialize_interrupt_controller(self) -> RpividInterruptInitSnapshot {
        let before = self.intc.read32(ARG_IC_ICTRL);

        // Match the RPIVID hardware bring-up sequence used by Raspberry Pi:
        // expose only the two HEVC phase-completion sources first. The hardware
        // does not latch their pending bits until the corresponding enable bits
        // are set.
        self.intc
            .write32(ARG_IC_ICTRL, ARG_IC_ICTRL_HEVC_ENABLE_MASK);
        let enabled = self.intc.read32(ARG_IC_ICTRL);

        assert_eq!(
            enabled & ARG_IC_ICTRL_HEVC_ENABLE_MASK,
            ARG_IC_ICTRL_HEVC_ENABLE_MASK,
            "BCM2711 HEVC interrupt controller did not enable Active1/Active2"
        );

        // The interrupt-latch bits are write-one-to-clear. Writing the value we
        // just read therefore clears anything that became pending as the HEVC
        // sources were enabled, while retaining the enable bits.
        self.intc.write32(ARG_IC_ICTRL, enabled);
        let after_clear = self.intc.read32(ARG_IC_ICTRL);

        assert_eq!(
            after_clear & ARG_IC_ICTRL_HEVC_ENABLE_MASK,
            ARG_IC_ICTRL_HEVC_ENABLE_MASK,
            "BCM2711 HEVC interrupt controller lost Active1/Active2 enables"
        );

        let snapshot = RpividInterruptInitSnapshot {
            before,
            enabled,
            after_clear,
        };

        info!(
            "BCM2711 HEVC interrupt controller initialized: before={:#010x}, enabled={:#010x}, after_clear={:#010x}, pending={:#010x}",
            snapshot.before,
            snapshot.enabled,
            snapshot.after_clear,
            snapshot.after_clear & ARG_IC_ICTRL_ALL_IRQ_MASK,
        );

        snapshot
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
