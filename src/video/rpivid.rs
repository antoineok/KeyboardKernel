//! Low-level register access for the BCM2711 HEVC decoder (RPIVID).
//!
//! The MMU identity-maps the BCM2711 peripheral window as Device-nGnRE, so the
//! physical addresses discovered from the FDT can be used directly as virtual
//! addresses after EL1 translation is enabled.

use core::{
    cell::UnsafeCell,
    ptr::{read_volatile, write_volatile},
};

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
const RPIVID_PHASE_BUFFER_COUNT: usize = 3;
const RPIVID_BITSTREAM_STAGING_BYTES: usize = 4 * 1024 * 1024;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividBitstreamWindow {
    base: u32,
    byte_offset: u8,
    byte_len: u32,
}

impl RpividBitstreamWindow {
    fn from_phys_range(phys_addr: u64, byte_len: usize) -> Self {
        assert!(byte_len != 0, "RPIVID bitstream window must not be empty");
        assert!(byte_len <= u32::MAX as usize, "RPIVID bitstream window is too large");

        let base = phys_addr >> 6;
        assert!(base <= u32::MAX as u64, "RPIVID bitstream address exceeds hardware register width");

        Self {
            base: base as u32,
            byte_offset: (phys_addr & 63) as u8,
            byte_len: byte_len as u32,
        }
    }

    #[inline(always)]
    pub const fn base_register_value(self) -> u32 {
        self.base
    }

    #[inline(always)]
    pub const fn byte_offset(self) -> u8 {
        self.byte_offset
    }

    #[inline(always)]
    pub const fn byte_len(self) -> u32 {
        self.byte_len
    }
}

pub struct RpividBitstreamBuffer {
    dma: DmaBuffer,
    used: usize,
}

impl RpividBitstreamBuffer {
    pub fn allocate() -> Self {
        let dma = DmaBuffer::allocate(RPIVID_BITSTREAM_STAGING_BYTES, RPIVID_DMA_GRANULE)
            .expect("failed to allocate RPIVID bitstream DMA buffer");

        Self { dma, used: 0 }
    }

    pub fn load(&mut self, data: &[u8]) {
        assert!(data.len() <= self.dma.len(), "RPIVID bitstream exceeds staging capacity");

        self.dma.as_mut_slice()[..data.len()].copy_from_slice(data);
        self.used = data.len();
        self.dma.sync_range_for_device(0, self.used);
    }

    pub fn window(&self, offset: usize, byte_len: usize) -> RpividBitstreamWindow {
        let end = offset.checked_add(byte_len).expect("RPIVID bitstream range overflow");
        assert!(end <= self.used, "RPIVID bitstream range exceeds loaded data");

        RpividBitstreamWindow::from_phys_range(self.dma.phys_addr() + offset as u64, byte_len)
    }

    #[inline(always)]
    pub fn phys_addr(&self) -> u64 {
        self.dma.phys_addr()
    }

    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        RPIVID_BITSTREAM_STAGING_BYTES
    }

    #[inline(always)]
    pub const fn used(&self) -> usize {
        self.used
    }
}

pub struct RpividWorkspaceStatic(UnsafeCell<Option<RpividDecodeWorkspace>>);

unsafe impl Sync for RpividWorkspaceStatic {}

impl RpividWorkspaceStatic {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    fn get_mut(&self) -> &mut Option<RpividDecodeWorkspace> {
        unsafe { &mut *self.0.get() }
    }

    pub fn stage_bitstream(&self, data: &[u8]) -> RpividBitstreamWindow {
        assert_eq!(
            crate::arch::cpu::current_id(),
            crate::arch::cpu::CpuId::BOOT,
            "RPIVID bitstream staging currently belongs to CPU0",
        );
        let workspace = self
            .get_mut()
            .as_mut()
            .expect("RPIVID workspace used before initialization");
        workspace.bitstream.load(data);
        workspace.bitstream.window(0, data.len())
    }
}

pub static RPIVID_WORKSPACE: RpividWorkspaceStatic = RpividWorkspaceStatic::new();

fn round_up_workspace_size(bytes: usize) -> usize {
    // Match hevc_d_round_up_size() from Raspberry Pi's RPIVID driver. The
    // decoder grows these buffers in a 3x/4x power-of-two progression.
    let n = if bytes < 256 {
        8
    } else {
        (usize::BITS - 1 - bytes.leading_zeros()) as usize
    };

    let three = 3usize
        .checked_shl(n as u32)
        .expect("RPIVID workspace size overflow");
    let four = 4usize
        .checked_shl(n as u32)
        .expect("RPIVID workspace size overflow");

    if bytes >= three { four } else { three }
}

pub struct RpividDecodeWorkspace {
    command: RpividCommandBuffer,
    bitstream: RpividBitstreamBuffer,
    pu: [DmaBuffer; RPIVID_PHASE_BUFFER_COUNT],
    coeff: [DmaBuffer; RPIVID_PHASE_BUFFER_COUNT],
    pu_size: usize,
    coeff_size: usize,
}

impl RpividDecodeWorkspace {
    pub fn allocate(width: usize, height: usize) -> Self {
        assert!(width != 0 && height != 0, "RPIVID dimensions must be non-zero");
        assert!(width <= 4096 && height <= 4096, "RPIVID dimensions exceed hardware limit");

        let pixels = width
            .checked_mul(height)
            .expect("RPIVID frame dimensions overflow");
        let coeff_size = round_up_workspace_size(pixels);
        let pu_size = round_up_workspace_size(pixels / 4);

        let command = RpividCommandBuffer::allocate();
        let bitstream = RpividBitstreamBuffer::allocate();
        let pu = core::array::from_fn(|_| {
            DmaBuffer::allocate(pu_size, RPIVID_DMA_GRANULE)
                .expect("failed to allocate RPIVID PU DMA buffer")
        });
        let coeff = core::array::from_fn(|_| {
            DmaBuffer::allocate(coeff_size, RPIVID_DMA_GRANULE)
                .expect("failed to allocate RPIVID coefficient DMA buffer")
        });

        Self {
            command,
            bitstream,
            pu,
            coeff,
            pu_size,
            coeff_size,
        }
    }

    pub fn total_bytes(&self) -> usize {
        RPIVID_COMMAND_BYTES
            + RPIVID_BITSTREAM_STAGING_BYTES
            + self.pu_size * RPIVID_PHASE_BUFFER_COUNT
            + self.coeff_size * RPIVID_PHASE_BUFFER_COUNT
    }

    fn validate_dma_encoding(&self) {
        assert_eq!(
            self.command.phys_addr() as usize % RPIVID_DMA_GRANULE,
            0,
            "RPIVID command buffer is not 64-byte aligned"
        );
        assert_eq!(self.command.command_capacity(), RPIVID_COMMAND_CAPACITY);
        assert_eq!(
            self.command.hardware_len().register_value(),
            (RPIVID_COMMAND_BYTES / RPIVID_DMA_GRANULE) as u32
        );
        assert_eq!(
            self.bitstream.phys_addr() as usize % RPIVID_DMA_GRANULE,
            0,
            "RPIVID bitstream buffer is not 64-byte aligned"
        );
        assert_eq!(self.bitstream.capacity(), RPIVID_BITSTREAM_STAGING_BYTES);

        for buffer in self.pu.iter().chain(self.coeff.iter()) {
            assert_eq!(
                buffer.phys_addr() as usize % RPIVID_DMA_GRANULE,
                0,
                "RPIVID workspace buffer is not 64-byte aligned"
            );
            let _ = RpividDmaAddress::from_phys(buffer.phys_addr());
            let _ = RpividDmaLength::from_bytes(buffer.len());
        }
    }
}

pub fn initialize_decode_workspace(width: usize, height: usize) {
    assert_eq!(
        crate::arch::cpu::current_id(),
        crate::arch::cpu::CpuId::BOOT,
        "RPIVID workspace must be initialized on CPU0",
    );
    assert!(
        RPIVID_WORKSPACE.get_mut().is_none(),
        "RPIVID workspace initialized more than once",
    );

    let mut workspace = RpividDecodeWorkspace::allocate(width, height);
    workspace.validate_dma_encoding();

    // The command FIFO and bitstream staging area are CPU-populated inputs.
    // PU and coefficient memory become device-write targets during decode.
    workspace.command.sync_for_device();

    let mut probe = [0u8; 256];
    for (index, byte) in probe.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(29).wrapping_add(0x31);
    }
    workspace.bitstream.load(&probe);

    // Real HEVC slice data need not begin on a 64-byte boundary. RPIVID
    // truncates BFBASE to 64-byte units and receives the low address bits in
    // BFCONTROL, so validate that representation with an intentionally
    // unaligned window.
    let probe_offset = 37usize;
    let probe_len = 173usize;
    let window = workspace.bitstream.window(probe_offset, probe_len);
    let reconstructed = ((window.base_register_value() as u64) << 6)
        + window.byte_offset() as u64;
    assert_eq!(
        reconstructed,
        workspace.bitstream.phys_addr() + probe_offset as u64,
    );
    assert_eq!(window.byte_len(), probe_len as u32);

    info!(
        "RPIVID decode workspace ready: {}x{}, command={} KiB, bitstream={} KiB, PU={}x{} KiB, coeff={}x{} KiB, total={} KiB, bitstream_phys={:#018x}, probe_base={:#010x}, probe_offset={}, probe_len={}, pu0={:#018x}, coeff0={:#018x}",
        width,
        height,
        RPIVID_COMMAND_BYTES / 1024,
        RPIVID_BITSTREAM_STAGING_BYTES / 1024,
        RPIVID_PHASE_BUFFER_COUNT,
        workspace.pu_size / 1024,
        RPIVID_PHASE_BUFFER_COUNT,
        workspace.coeff_size / 1024,
        workspace.total_bytes() / 1024,
        workspace.bitstream.phys_addr(),
        window.base_register_value(),
        window.byte_offset(),
        window.byte_len(),
        workspace.pu[0].phys_addr(),
        workspace.coeff[0].phys_addr(),
    );

    *RPIVID_WORKSPACE.get_mut() = Some(workspace);
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
