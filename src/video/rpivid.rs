//! Low-level register access for the BCM2711 HEVC decoder (RPIVID).
//!
//! The MMU identity-maps the BCM2711 peripheral window as Device-nGnRE, so the
//! physical addresses discovered from the FDT can be used directly as virtual
//! addresses after EL1 translation is enabled.

use alloc::vec::Vec;
use core::{
    cell::UnsafeCell,
    hint::spin_loop,
    ptr::{read_volatile, write_volatile},
    time::Duration,
};

use crate::{info, memory::dma::DmaBuffer};

use super::{hevc::HevcResources, hevc_syntax::FirstPictureSyntax};

// Phase-1 HEVC register offsets.
const RPI_SPS0: usize = 0x0000;
const RPI_SPS1: usize = 0x0004;
const RPI_PPS: usize = 0x0008;
const RPI_SLICE: usize = 0x000c;
const RPI_TILESTART: usize = 0x0010;
const RPI_TILEEND: usize = 0x0014;
const RPI_SLICESTART: usize = 0x0018;
const RPI_MODE: usize = 0x001c;
const RPI_QP: usize = 0x0030;
const RPI_CONTROL: usize = 0x0034;
const RPI_STATUS: usize = 0x0038;
const RPI_VERSION: usize = 0x003c;
const RPI_BFBASE: usize = 0x0040;
const RPI_BFNUM: usize = 0x0044;
const RPI_BFCONTROL: usize = 0x0048;
const RPI_BFSTATUS: usize = 0x004c;
const RPI_PUWBASE: usize = 0x0050;
const RPI_PUWSTRIDE: usize = 0x0054;
const RPI_COEFFWBASE: usize = 0x0058;
const RPI_COEFFWSTRIDE: usize = 0x005c;
const RPI_SLICECMDS: usize = 0x0060;
const RPI_BEGINTILEEND: usize = 0x0064;
const RPI_TRANSFER: usize = 0x0068;
const RPI_CFBASE: usize = 0x006c;
const RPI_CFNUM: usize = 0x0070;
const RPI_CFSTATUS: usize = 0x0074;

// Phase-2 HEVC register offsets.
const RPI_PURBASE: usize = 0x8000;
const RPI_PURSTRIDE: usize = 0x8004;
const RPI_COEFFRBASE: usize = 0x8008;
const RPI_COEFFRSTRIDE: usize = 0x800c;
const RPI_NUMROWS: usize = 0x8010;
const RPI_CONFIG2: usize = 0x8014;
const RPI_OUTYBASE: usize = 0x8018;
const RPI_OUTYSTRIDE: usize = 0x801c;
const RPI_OUTCBASE: usize = 0x8020;
const RPI_OUTCSTRIDE: usize = 0x8024;
const RPI_STATUS2: usize = 0x8028;
const RPI_FRAMESIZE: usize = 0x802c;
const RPI_MVBASE: usize = 0x8030;
const RPI_MVSTRIDE: usize = 0x8034;
const RPI_COLBASE: usize = 0x8038;
const RPI_COLSTRIDE: usize = 0x803c;
const RPI_CURRPOC: usize = 0x8040;
const RPI_REF_BASE: usize = 0x9000;
const RPI_REF_STRIDE: usize = 0x10;

// Local Argon interrupt controller register block.
const ARG_IC_ICTRL: usize = 0x0000;

const ARG_IC_ICTRL_ACTIVE1_INT: u32 = 1 << 0;
const ARG_IC_ICTRL_ACTIVE1_EN: u32 = 1 << 2;
const ARG_IC_ICTRL_ACTIVE2_INT: u32 = 1 << 4;
const ARG_IC_ICTRL_ACTIVE2_EN: u32 = 1 << 6;
const ARG_IC_ICTRL_H264_INTERRUPT_INT: u32 = 1 << 25;
const ARG_IC_ICTRL_VP9_INTERRUPT_INT: u32 = 1 << 29;

const ARG_IC_ICTRL_HEVC_ENABLE_MASK: u32 = ARG_IC_ICTRL_ACTIVE1_EN | ARG_IC_ICTRL_ACTIVE2_EN;
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

const PROB_BACKUP: u32 = (20 << 12) | (20 << 6);
const PAUSE_MODE_TILE: u32 = 0xffff;

// HEVC Main I-slice CABAC initialisation values (H.265 section 9.3.2.2).
const PROB_INIT_I: [u8; 156] = [
    153, 200, 139, 141, 157, 154, 154, 154, 154, 154, 184, 154, 154, 154, 184, 63, 154, 154, 154,
    154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 153, 138, 138, 111, 141, 94, 138, 182, 154,
    154, 154, 140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140,
    179, 166, 182, 140, 227, 122, 197, 110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143,
    127, 111, 79, 108, 123, 63, 110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127,
    111, 79, 108, 123, 63, 91, 171, 134, 141, 138, 153, 136, 167, 152, 152, 139, 139, 111, 111,
    125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125,
    107, 125, 141, 179, 153, 125, 140, 139, 182, 182, 152, 136, 152, 136, 153, 136, 139, 111, 136,
    139, 111, 0, 0,
];

// HEVC Main P-slice CABAC initialisation values (H.265 section 9.3.2.2).
// This is prob_init[1] from Raspberry Pi's RPIVID driver.
const PROB_INIT_P: [u8; 156] = [
    153, 185, 107, 139, 126, 197, 185, 201, 154, 149, 154, 139, 154, 154, 154, 152, 110, 122, 95,
    79, 63, 31, 31, 153, 153, 168, 140, 198, 79, 124, 138, 94, 153, 111, 149, 107, 167, 154, 154,
    154, 154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137, 169, 194,
    166, 167, 154, 167, 137, 182, 125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95,
    94, 108, 123, 108, 125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108,
    123, 108, 121, 140, 61, 154, 107, 167, 91, 122, 107, 167, 139, 139, 155, 154, 139, 153, 139,
    123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 166, 183, 140,
    136, 153, 154, 170, 153, 123, 123, 107, 121, 107, 121, 167, 151, 183, 140, 151, 183, 140, 0, 0,
];

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
    used_commands: usize,
}

impl RpividCommandBuffer {
    pub fn allocate() -> Self {
        let dma = DmaBuffer::allocate(RPIVID_COMMAND_BYTES, RPIVID_DMA_GRANULE)
            .expect("failed to allocate RPIVID command DMA buffer");

        Self {
            dma,
            used_commands: 0,
        }
    }

    pub fn reset(&mut self) {
        self.used_commands = 0;
    }

    pub fn push(&mut self, register: usize, data: u32) {
        assert_eq!(
            register & 3,
            0,
            "RPIVID command register is not 32-bit aligned"
        );
        assert!(
            register <= u32::MAX as usize,
            "RPIVID command register exceeds u32"
        );
        assert!(
            self.used_commands < RPIVID_COMMAND_CAPACITY,
            "RPIVID command FIFO overflow",
        );

        let offset = self.used_commands * core::mem::size_of::<u64>();
        let bytes = self.dma.as_mut_slice();
        bytes[offset..offset + 4].copy_from_slice(&(register as u32).to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&data.to_le_bytes());
        self.used_commands += 1;
    }

    #[inline(always)]
    pub const fn used_commands(&self) -> usize {
        self.used_commands
    }

    pub fn sync_used_for_device(&self) {
        self.dma
            .sync_range_for_device(0, self.used_commands * core::mem::size_of::<u64>());
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
        assert!(
            byte_len <= u32::MAX as usize,
            "RPIVID bitstream window is too large"
        );

        let base = phys_addr >> 6;
        assert!(
            base <= u32::MAX as u64,
            "RPIVID bitstream address exceeds hardware register width"
        );

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
        assert!(
            data.len() <= self.dma.len(),
            "RPIVID bitstream exceeds staging capacity"
        );

        self.dma.as_mut_slice()[..data.len()].copy_from_slice(data);
        self.used = data.len();
        self.dma.sync_range_for_device(0, self.used);
    }

    pub fn window(&self, offset: usize, byte_len: usize) -> RpividBitstreamWindow {
        let end = offset
            .checked_add(byte_len)
            .expect("RPIVID bitstream range overflow");
        assert!(
            end <= self.used,
            "RPIVID bitstream range exceeds loaded data"
        );

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividPhase1Snapshot {
    pub command_count: u32,
    pub irq_received: bool,
    pub cfstatus: u32,
    pub cfnum: u32,
    pub status: u32,
    pub bfstatus: u32,
    pub interrupt_control: u32,
    pub pu_stride: u32,
    pub coeff_stride: u32,
}

impl RpividPhase1Snapshot {
    pub const fn complete(self) -> bool {
        self.irq_received && self.cfstatus == self.cfnum
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RpividPhase2Snapshot {
    pub irq_received: bool,
    pub status2: u32,
    pub interrupt_control: u32,
    pub config2: u32,
    pub coded_width: u32,
    pub coded_height: u32,
    pub storage_width: u32,
    pub storage_height: u32,
    pub frame_stride: u32,
    pub chroma_offset: u32,
    pub frame_bytes: u32,
    pub colmv_stride: u32,
    pub colmv_bytes: u32,
    pub output_checksum: u64,
    pub nonzero_bytes: u32,
}

impl RpividPhase2Snapshot {
    pub const fn complete(self) -> bool {
        self.irq_received
    }
}

pub struct RpividDecodedFrame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    pitch: usize,
}

impl RpividDecodedFrame {
    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn pitch(&self) -> usize {
        self.pitch
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
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
            crate::arch::cpu::CpuId::new(1).expect("CPU1 must exist"),
            "RPIVID bitstream staging belongs to CPU1",
        );
        let workspace = self
            .get_mut()
            .as_mut()
            .expect("RPIVID workspace used before initialization");
        workspace.bitstream.load(data);
        workspace.bitstream.window(0, data.len())
    }

    pub fn submit_picture_phase1(
        &self,
        picture: &FirstPictureSyntax,
        cabac_ebsp: &[u8],
    ) -> RpividPhase1Snapshot {
        assert_eq!(
            crate::arch::cpu::current_id(),
            crate::arch::cpu::CpuId::new(1).expect("CPU1 must exist"),
            "RPIVID phase 1 belongs to CPU1",
        );
        self.get_mut()
            .as_mut()
            .expect("RPIVID workspace used before initialization")
            .submit_picture_phase1(picture, cabac_ebsp)
    }

    pub fn submit_picture_phase2(
        &self,
        picture: &FirstPictureSyntax,
        picture_index: usize,
    ) -> RpividPhase2Snapshot {
        assert_eq!(
            crate::arch::cpu::current_id(),
            crate::arch::cpu::CpuId::new(1).expect("CPU1 must exist"),
            "RPIVID phase 2 belongs to CPU1",
        );
        self.get_mut()
            .as_mut()
            .expect("RPIVID workspace used before initialization")
            .submit_picture_phase2(picture, picture_index)
    }

    pub fn copy_frame_rgbx(
        &self,
        picture: &FirstPictureSyntax,
        picture_index: usize,
    ) -> RpividDecodedFrame {
        self.copy_frame_rgbx_reuse(picture, picture_index, Vec::new())
    }

    pub fn copy_frame_rgbx_reuse(
        &self,
        picture: &FirstPictureSyntax,
        picture_index: usize,
        pixels: Vec<u8>,
    ) -> RpividDecodedFrame {
        assert_eq!(
            crate::arch::cpu::current_id(),
            crate::arch::cpu::CpuId::new(1).expect("CPU1 must exist"),
            "RPIVID frame conversion belongs to CPU1",
        );
        self.get_mut()
            .as_mut()
            .expect("RPIVID workspace used before initialization")
            .copy_frame_rgbx_reuse(picture, picture_index, pixels)
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

    if bytes >= three {
        four
    } else {
        three
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RpividFrameLayout {
    coded_width: usize,
    coded_height: usize,
    storage_width: usize,
    storage_height: usize,
    frame_stride: usize,
    chroma_offset: usize,
    frame_bytes: usize,
    colmv_stride: usize,
    colmv_bytes: usize,
}

impl RpividFrameLayout {
    fn from_picture(picture: &FirstPictureSyntax) -> Self {
        let coded_width = picture.sps.width as usize;
        let coded_height = picture.sps.height as usize;
        let storage_width = align_up(coded_width, 128);
        let storage_height = align_up(coded_height, 16);
        let column_height = storage_height
            .checked_mul(3)
            .expect("RPIVID column height overflow")
            / 2;
        let frame_stride = column_height
            .checked_mul(128)
            .expect("RPIVID column stride overflow");
        let chroma_offset = storage_height
            .checked_mul(128)
            .expect("RPIVID chroma offset overflow");
        let frame_bytes = column_height
            .checked_mul(storage_width)
            .expect("RPIVID frame allocation overflow");
        let colmv_stride = align_up(coded_width, 64);
        let colmv_rows = align_up(coded_height, 64) >> 4;
        let colmv_bytes = colmv_stride
            .checked_mul(colmv_rows)
            .expect("RPIVID motion-vector allocation overflow");

        Self {
            coded_width,
            coded_height,
            storage_width,
            storage_height,
            frame_stride,
            chroma_offset,
            frame_bytes,
            colmv_stride,
            colmv_bytes,
        }
    }
}

struct RpividFrameBuffer {
    dma: DmaBuffer,
    layout: RpividFrameLayout,
}

pub struct RpividDecodeWorkspace {
    command: RpividCommandBuffer,
    bitstream: RpividBitstreamBuffer,
    pu: [DmaBuffer; RPIVID_PHASE_BUFFER_COUNT],
    coeff: [DmaBuffer; RPIVID_PHASE_BUFFER_COUNT],
    pu_size: usize,
    coeff_size: usize,
    frames: [Option<RpividFrameBuffer>; 2],
}

impl RpividDecodeWorkspace {
    pub fn allocate(width: usize, height: usize) -> Self {
        assert!(
            width != 0 && height != 0,
            "RPIVID dimensions must be non-zero"
        );
        assert!(
            width <= 4096 && height <= 4096,
            "RPIVID dimensions exceed hardware limit"
        );

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
            frames: [None, None],
        }
    }

    fn submit_picture_phase1(
        &mut self,
        picture: &FirstPictureSyntax,
        cabac_ebsp: &[u8],
    ) -> RpividPhase1Snapshot {
        let sps = picture.sps;
        let pps = picture.pps;
        let slice = picture.slice;

        assert!(
            slice.first_slice_segment_in_pic,
            "kwv1 requires one slice per picture"
        );
        assert!(
            matches!(slice.slice_type, 1 | 2),
            "kwv1 RPIVID path only supports I/P slices"
        );
        assert!(
            !pps.tiles_enabled,
            "kwv1 RPIVID path does not support tiles"
        );
        assert!(
            !pps.entropy_coding_sync_enabled,
            "kwv1 RPIVID path requires WPP disabled"
        );
        assert!(
            !sps.scaling_list_enabled,
            "kwv1 RPIVID path does not support scaling lists"
        );
        assert!(
            !sps.temporal_mvp_enabled,
            "kwv1 RPIVID path requires temporal MVP disabled"
        );
        assert_eq!(
            slice.entry_point_count, 0,
            "kwv1 non-WPP slice must not contain entry points"
        );
        assert!(!cabac_ebsp.is_empty(), "RPIVID CABAC payload is empty");
        if slice.slice_type == 1 {
            assert_eq!(
                slice.num_ref_idx_l0_active, 1,
                "kwv1 P slice must use one L0 reference"
            );
            assert_eq!(
                slice.num_ref_idx_l1_active, 0,
                "kwv1 P slice must not use L1 references"
            );
            assert_eq!(
                slice.closest_negative_delta_poc, -1,
                "kwv1 P slice must reference the preceding picture"
            );
        }

        self.bitstream.load(cabac_ebsp);
        let bitstream = self.bitstream.window(0, cabac_ebsp.len());

        self.command.reset();
        build_picture_single_tile_commands(&mut self.command, picture, bitstream);
        self.command.sync_used_for_device();

        let phase_index = 0usize;
        let pu_stride = phase_stride(self.pu_size, picture.ctb_height as usize);
        let coeff_stride = phase_stride(self.coeff_size, picture.ctb_height as usize);
        self.pu[phase_index].sync_for_device();
        self.coeff[phase_index].sync_for_device();

        let registers = RpividRegisters::from_resources(super::hevc::HEVC_PLATFORM.resources());
        let _ = registers.acknowledge_interrupts();
        let irq_before = super::hevc::HEVC_PLATFORM.irq_count();

        registers.hevc.write32(
            RPI_PUWBASE,
            RpividDmaAddress::from_phys(self.pu[phase_index].phys_addr()).register_value(),
        );
        registers.hevc.write32(
            RPI_PUWSTRIDE,
            RpividDmaLength::from_bytes(pu_stride).register_value(),
        );
        registers.hevc.write32(
            RPI_COEFFWBASE,
            RpividDmaAddress::from_phys(self.coeff[phase_index].phys_addr()).register_value(),
        );
        registers.hevc.write32(
            RPI_COEFFWSTRIDE,
            RpividDmaLength::from_bytes(coeff_stride).register_value(),
        );
        registers
            .hevc
            .write32(RPI_CFNUM, self.command.used_commands() as u32);
        registers
            .hevc
            .write32(RPI_CFBASE, self.command.hardware_addr().register_value());

        let started = crate::time::time_manager().uptime();
        let timeout = Duration::from_millis(250);
        while super::hevc::HEVC_PLATFORM.irq_count() == irq_before
            && crate::time::time_manager().uptime().saturating_sub(started) < timeout
        {
            spin_loop();
        }

        let irq_received = super::hevc::HEVC_PLATFORM.irq_count() != irq_before;
        RpividPhase1Snapshot {
            command_count: self.command.used_commands() as u32,
            irq_received,
            cfstatus: registers.hevc.read32(RPI_CFSTATUS),
            cfnum: registers.hevc.read32(RPI_CFNUM),
            status: registers.hevc.read32(RPI_STATUS),
            bfstatus: registers.hevc.read32(RPI_BFSTATUS),
            interrupt_control: registers.intc.read32(ARG_IC_ICTRL),
            pu_stride: pu_stride as u32,
            coeff_stride: coeff_stride as u32,
        }
    }

    fn submit_picture_phase2(
        &mut self,
        picture: &FirstPictureSyntax,
        picture_index: usize,
    ) -> RpividPhase2Snapshot {
        let sps = picture.sps;
        let slice = picture.slice;
        assert!(
            matches!(slice.slice_type, 1 | 2),
            "kwv1 phase 2 only supports I/P slices"
        );
        assert_eq!(
            sps.bit_depth_luma_minus8, 0,
            "kwv1 phase 2 supports 8-bit luma only"
        );
        assert_eq!(
            sps.bit_depth_chroma_minus8, 0,
            "kwv1 phase 2 supports 8-bit chroma only"
        );
        assert_eq!(
            sps.chroma_format_idc, 1,
            "kwv1 phase 2 requires 4:2:0 chroma"
        );
        assert!(
            !sps.temporal_mvp_enabled,
            "kwv1 phase 2 requires temporal MVP disabled"
        );

        let layout = RpividFrameLayout::from_picture(picture);
        self.ensure_frame_buffers(layout);
        let current_index = picture_index & 1;
        let previous_index = current_index ^ 1;
        let is_p = slice.slice_type == 1;
        if is_p {
            assert!(picture_index != 0, "first kwv1 picture cannot be a P slice");
        }

        self.frames[current_index]
            .as_mut()
            .expect("RPIVID current frame buffer was not allocated")
            .dma
            .sync_for_device();

        let phase_index = 0usize;
        let pu_stride = phase_stride(self.pu_size, picture.ctb_height as usize);
        let coeff_stride = phase_stride(self.coeff_size, picture.ctb_height as usize);
        let config2 = phase2_config2(picture, false);
        let frame_size = (sps.height << 16) | sps.width;

        let current = self.frames[current_index]
            .as_ref()
            .expect("RPIVID current frame buffer disappeared");
        let frame_phys = current.dma.phys_addr();
        let chroma_phys = frame_phys + layout.chroma_offset as u64;
        let previous_phys = if is_p {
            self.frames[previous_index]
                .as_ref()
                .expect("RPIVID previous reference frame was not decoded")
                .dma
                .phys_addr()
        } else {
            frame_phys
        };
        let previous_chroma_phys = previous_phys + layout.chroma_offset as u64;

        let registers = RpividRegisters::from_resources(super::hevc::HEVC_PLATFORM.resources());
        let _ = registers.acknowledge_interrupts();
        let irq_before = super::hevc::HEVC_PLATFORM.irq_count();

        registers.hevc.write32(
            RPI_PURBASE,
            RpividDmaAddress::from_phys(self.pu[phase_index].phys_addr()).register_value(),
        );
        registers.hevc.write32(
            RPI_PURSTRIDE,
            RpividDmaLength::from_bytes(pu_stride).register_value(),
        );
        registers.hevc.write32(
            RPI_COEFFRBASE,
            RpividDmaAddress::from_phys(self.coeff[phase_index].phys_addr()).register_value(),
        );
        registers.hevc.write32(
            RPI_COEFFRSTRIDE,
            RpividDmaLength::from_bytes(coeff_stride).register_value(),
        );

        let frame_stride = RpividDmaLength::from_bytes(layout.frame_stride).register_value();
        registers.hevc.write32(
            RPI_OUTYBASE,
            RpividDmaAddress::from_phys(frame_phys).register_value(),
        );
        registers.hevc.write32(RPI_OUTYSTRIDE, frame_stride);
        registers.hevc.write32(
            RPI_OUTCBASE,
            RpividDmaAddress::from_phys(chroma_phys).register_value(),
        );
        registers.hevc.write32(RPI_OUTCSTRIDE, frame_stride);

        // Seed every slot with a valid surface. P slices use DPB slot 0 for
        // their sole L0 reference, which is always the immediately preceding
        // picture in the constrained kwv1 encoder profile.
        for index in 0..16usize {
            let base = RPI_REF_BASE + index * RPI_REF_STRIDE;
            registers.hevc.write32(
                base,
                RpividDmaAddress::from_phys(frame_phys).register_value(),
            );
            registers.hevc.write32(base + 4, frame_stride);
            registers.hevc.write32(
                base + 8,
                RpividDmaAddress::from_phys(chroma_phys).register_value(),
            );
            registers.hevc.write32(base + 12, frame_stride);
        }
        if is_p {
            registers.hevc.write32(
                RPI_REF_BASE,
                RpividDmaAddress::from_phys(previous_phys).register_value(),
            );
            registers.hevc.write32(RPI_REF_BASE + 4, frame_stride);
            registers.hevc.write32(
                RPI_REF_BASE + 8,
                RpividDmaAddress::from_phys(previous_chroma_phys).register_value(),
            );
            registers.hevc.write32(RPI_REF_BASE + 12, frame_stride);
        }

        registers.hevc.write32(RPI_CONFIG2, config2);
        registers.hevc.write32(RPI_FRAMESIZE, frame_size);
        registers.hevc.write32(RPI_CURRPOC, slice.pic_order_cnt_lsb);

        let colmv_stride = RpividDmaLength::from_bytes(layout.colmv_stride).register_value();
        registers.hevc.write32(RPI_COLSTRIDE, colmv_stride);
        registers.hevc.write32(RPI_MVSTRIDE, colmv_stride);
        registers.hevc.write32(RPI_MVBASE, 0);
        registers.hevc.write32(RPI_COLBASE, 0);

        registers.hevc.write32(RPI_NUMROWS, picture.ctb_height);

        let started = crate::time::time_manager().uptime();
        let timeout = Duration::from_millis(250);
        while super::hevc::HEVC_PLATFORM.irq_count() == irq_before
            && crate::time::time_manager().uptime().saturating_sub(started) < timeout
        {
            spin_loop();
        }

        let irq_received = super::hevc::HEVC_PLATFORM.irq_count() != irq_before;
        if irq_received {
            self.frames[current_index]
                .as_mut()
                .expect("RPIVID current frame buffer disappeared after phase 2")
                .dma
                .sync_for_cpu();
        }

        RpividPhase2Snapshot {
            irq_received,
            status2: registers.hevc.read32(RPI_STATUS2),
            interrupt_control: registers.intc.read32(ARG_IC_ICTRL),
            config2,
            coded_width: layout.coded_width as u32,
            coded_height: layout.coded_height as u32,
            storage_width: layout.storage_width as u32,
            storage_height: layout.storage_height as u32,
            frame_stride: layout.frame_stride as u32,
            chroma_offset: layout.chroma_offset as u32,
            frame_bytes: layout.frame_bytes as u32,
            colmv_stride: layout.colmv_stride as u32,
            colmv_bytes: 0,
            output_checksum: 0,
            nonzero_bytes: 0,
        }
    }

    fn ensure_frame_buffers(&mut self, layout: RpividFrameLayout) {
        for frame in &mut self.frames {
            let matches = frame
                .as_ref()
                .map(|existing| existing.layout == layout)
                .unwrap_or(false);
            if !matches {
                *frame = Some(RpividFrameBuffer {
                    dma: DmaBuffer::allocate(layout.frame_bytes, RPIVID_DMA_GRANULE)
                        .expect("failed to allocate RPIVID decoded-frame DMA buffer"),
                    layout,
                });
            }
        }
    }

    fn copy_frame_rgbx_reuse(
        &mut self,
        picture: &FirstPictureSyntax,
        picture_index: usize,
        mut pixels: Vec<u8>,
    ) -> RpividDecodedFrame {
        let sps = picture.sps;
        let frame = self.frames[picture_index & 1]
            .as_ref()
            .expect("RPIVID decoded frame requested before phase 2");
        let layout = frame.layout;
        assert_eq!(layout, RpividFrameLayout::from_picture(picture));

        let visible_width = sps.visible_width() as usize;
        let visible_height = sps.visible_height() as usize;
        let pitch = visible_width
            .checked_mul(4)
            .expect("decoded RGBX pitch overflow");
        let len = pitch
            .checked_mul(visible_height)
            .expect("decoded RGBX allocation overflow");
        pixels.resize(len, 0);
        let source = frame.dma.as_slice();

        assert_eq!(
            visible_width & 1,
            0,
            "kwv1 4:2:0 visible width must be even"
        );
        assert_eq!(
            (sps.crop_left as usize) & 1,
            0,
            "kwv1 4:2:0 left crop must be even"
        );

        for y in 0..visible_height {
            let source_y = y + sps.crop_top as usize;
            let mut x = 0usize;
            while x < visible_width {
                let source_x = x + sps.crop_left as usize;
                let column = source_x / 128;
                let in_column = source_x & 127;
                let column_base = column * layout.frame_stride;
                let y_offset = column_base + source_y * 128 + in_column;
                let uv_offset =
                    column_base + layout.chroma_offset + (source_y / 2) * 128 + in_column;
                let cb = source[uv_offset];
                let cr = source[uv_offset + 1];
                let dest = y * pitch + x * 4;
                write_limited_bt709_pair(
                    &mut pixels[dest..dest + 8],
                    source[y_offset],
                    source[y_offset + 1],
                    cb,
                    cr,
                );
                x += 2;
            }
        }

        RpividDecodedFrame {
            pixels,
            width: visible_width as u32,
            height: visible_height as u32,
            pitch,
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

fn align_up(value: usize, alignment: usize) -> usize {
    assert!(
        alignment.is_power_of_two(),
        "RPIVID alignment must be a power of two"
    );
    value
        .checked_add(alignment - 1)
        .expect("RPIVID alignment overflow")
        & !(alignment - 1)
}

fn is_reference_nal(nal_unit_type: u8) -> bool {
    (u32::from(nal_unit_type) & !0x0e) != 0
}

fn phase2_config2(picture: &FirstPictureSyntax, write_motion_vectors: bool) -> u32 {
    let sps = picture.sps;
    let pps = picture.pps;
    let mut config = (sps.bit_depth_luma_minus8 + 8)
        | ((sps.bit_depth_chroma_minus8 + 8) << 4)
        | (sps.log2_ctb_size << 10)
        | ((pps.constrained_intra_pred as u32) << 13)
        | ((sps.strong_intra_smoothing_enabled as u32) << 14)
        | ((write_motion_vectors as u32) << 15)
        | ((pps.log2_parallel_merge_level_minus2 + 2) << 16)
        | ((sps.pcm_loop_filter_disabled as u32) << 20)
        | (((pps.pps_cb_qp_offset as u32) & 31) << 21)
        | (((pps.pps_cr_qp_offset as u32) & 31) << 26);
    if sps.bit_depth_luma_minus8 != 0 {
        config |= 1 << 8;
    }
    if sps.bit_depth_chroma_minus8 != 0 {
        config |= 1 << 9;
    }
    // kwv1 disables temporal MVP, so CONFIG2 bit 19 remains clear.
    config
}

#[inline(always)]
fn write_limited_bt709_pair(destination: &mut [u8], y0: u8, y1: u8, cb: u8, cr: u8) {
    debug_assert!(destination.len() >= 8);
    let d = i32::from(cb) - 128;
    let e = i32::from(cr) - 128;
    let red_chroma = 459 * e + 128;
    let green_chroma = -55 * d - 136 * e + 128;
    let blue_chroma = 541 * d + 128;

    for (index, y) in [y0, y1].into_iter().enumerate() {
        let c = (i32::from(y) - 16).max(0);
        let luma = 298 * c;
        let offset = index * 4;
        destination[offset] = clamp_u8((luma + red_chroma) >> 8);
        destination[offset + 1] = clamp_u8((luma + green_chroma) >> 8);
        destination[offset + 2] = clamp_u8((luma + blue_chroma) >> 8);
        destination[offset + 3] = 0xff;
    }
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn phase_stride(buffer_size: usize, ctb_height: usize) -> usize {
    assert!(ctb_height != 0, "RPIVID CTB height is zero");
    let stride = (buffer_size / ctb_height) & !(RPIVID_DMA_GRANULE - 1);
    assert!(stride != 0, "RPIVID phase stride rounded to zero");
    stride
}

fn build_picture_single_tile_commands(
    commands: &mut RpividCommandBuffer,
    picture: &FirstPictureSyntax,
    bitstream: RpividBitstreamWindow,
) {
    let sps = picture.sps;
    let pps = picture.pps;
    let slice = picture.slice;
    let slice_qp = 26 + pps.init_qp_minus26 + slice.slice_qp_delta;
    assert!(
        (0..=51).contains(&slice_qp),
        "RPIVID slice QP is outside 0..=51"
    );

    commands.push(RPI_BFBASE, bitstream.base_register_value());
    commands.push(RPI_BFNUM, bitstream.byte_len());
    commands.push(RPI_BFCONTROL, bitstream.byte_offset() as u32 | (1 << 7));
    // Match Raspberry Pi's driver: feed the original EBSP bytes and let
    // RPIVID remove emulation-prevention bytes in hardware.
    commands.push(RPI_BFCONTROL, bitstream.byte_offset() as u32 | (1 << 6));

    write_slice_probabilities(commands, slice_qp, slice.slice_type);
    write_slice_messages(commands, picture);
    write_slice_segment_registers(commands, picture);

    let slice_const = (slice.max_num_merge_cand << 0)
        | (slice.num_ref_idx_l0_active << 4)
        | (slice.num_ref_idx_l1_active << 8)
        | (slice.slice_type << 12)
        | ((slice.sao_luma as u32) << 14)
        | ((slice.sao_chroma as u32) << 15);

    let last_x = picture.ctb_width - 1;
    let last_y = picture.ctb_height - 1;
    let tile_end = last_x | (last_y << 16);

    // kwv1 uses exactly one independent slice and one implicit tile spanning
    // the complete picture. This mirrors RPIVID's non-WPP decode_slice path.
    commands.push(RPI_TILESTART, 0);
    commands.push(RPI_TILEEND, tile_end);
    commands.push(RPI_BEGINTILEEND, tile_end);
    commands.push(RPI_SLICE, slice_register(picture, last_y, slice_const));
    commands.push(RPI_QP, 6 * sps.bit_depth_luma_minus8 + slice_qp as u32);
    commands.push(RPI_MODE, PAUSE_MODE_TILE | (1 << 17) | (1 << 18));
    commands.push(RPI_CONTROL, 0);
    commands.push(RPI_STATUS, 1 | (last_x << 5) | (last_y << 18));
}

fn write_slice_probabilities(commands: &mut RpividCommandBuffer, slice_qp: i32, slice_type: u32) {
    let init = match slice_type {
        2 => &PROB_INIT_I,
        1 => &PROB_INIT_P,
        _ => panic!("kwv1 CABAC initialization only supports I/P slices"),
    };
    let q = slice_qp.clamp(0, 51);
    let mut probabilities = [0u8; 156];
    for index in 0..154 {
        let init_value = init[index] as i32;
        let m = (init_value >> 4) * 5 - 45;
        let n = ((init_value & 15) << 3) - 16;
        let mut pre = 2 * (((m * q) >> 4) + n) - 127;
        pre ^= pre >> 31;
        if pre > 124 {
            pre = 124 + (pre & 1);
        }
        probabilities[index] = pre as u8;
    }

    for offset in (0..probabilities.len()).step_by(4) {
        let word = u32::from(probabilities[offset])
            | (u32::from(probabilities[offset + 1]) << 8)
            | (u32::from(probabilities[offset + 2]) << 16)
            | (u32::from(probabilities[offset + 3]) << 24);
        commands.push(0x1000 + offset, word);
    }
    commands.push(RPI_TRANSFER, PROB_BACKUP);
}

fn write_slice_messages(commands: &mut RpividCommandBuffer, picture: &FirstPictureSyntax) {
    let pps = picture.pps;
    let slice = picture.slice;
    let mut messages = [0u32; 5];
    let mut count = 0usize;

    let mut cmd_slice = match slice.slice_type {
        2 => 1u32,
        1 => 2u32,
        _ => panic!("kwv1 slice messages only support I/P slices"),
    };
    if slice.slice_type == 1 {
        cmd_slice |= 1 << 2; // one L0 reference
        cmd_slice |= slice.max_num_merge_cand << 11;
        cmd_slice |= 1 << 10; // NoBackwardPredictionFlag: sole ref is in the past
    }
    // With temporal MVP disabled the hardware treats the collocated source as
    // L0. Keep the bit set for both I and P pictures, matching the Linux path.
    cmd_slice |= 1 << 14;
    messages[count] = cmd_slice;
    count += 1;

    if slice.slice_type == 1 {
        messages[count] = 0; // DPB slot 0, short-term, unweighted
        count += 1;
        messages[count] = slice.pic_order_cnt_lsb.wrapping_sub(1) & 0xffff;
        count += 1;
    }

    messages[count] = ((slice.beta_offset_div2 as u32) & 15)
        | (((slice.tc_offset_div2 as u32) & 15) << 4)
        | ((slice.deblocking_filter_disabled as u32) << 8)
        | ((slice.loop_filter_across_slices_enabled as u32) << 9)
        | ((pps.loop_filter_across_tiles_enabled as u32) << 10);
    count += 1;
    messages[count] =
        (((slice.slice_cr_qp_offset as u32) & 31) << 5) | ((slice.slice_cb_qp_offset as u32) & 31);
    count += 1;

    commands.push(
        RPI_SLICECMDS,
        u32::try_from(count).expect("slice command count does not fit in u32"),
    );
    for (index, &message) in messages[..count].iter().enumerate() {
        commands.push(0x4000 + index * 4, message);
    }
}

fn write_slice_segment_registers(commands: &mut RpividCommandBuffer, picture: &FirstPictureSyntax) {
    let sps = picture.sps;
    let pps = picture.pps;
    let slice = picture.slice;

    let sps0 = (sps.log2_min_luma_coding_block_size << 0)
        | (sps.log2_ctb_size << 4)
        | (sps.log2_min_luma_transform_block_size << 8)
        | (sps.log2_max_luma_transform_block_size << 12)
        | ((sps.bit_depth_luma_minus8 + 8) << 16)
        | ((sps.bit_depth_chroma_minus8 + 8) << 20)
        | (sps.max_transform_hierarchy_depth_intra << 24)
        | (sps.max_transform_hierarchy_depth_inter << 28);

    let pcm_luma = if sps.pcm_enabled {
        sps.pcm_sample_bit_depth_luma
    } else {
        1
    };
    let pcm_chroma = if sps.pcm_enabled {
        sps.pcm_sample_bit_depth_chroma
    } else {
        1
    };
    let pcm_min = if sps.pcm_enabled {
        sps.log2_min_pcm_luma_coding_block_size
    } else {
        3
    };
    let pcm_max = if sps.pcm_enabled {
        sps.log2_max_pcm_luma_coding_block_size
    } else {
        3
    };
    let chroma = if sps.separate_colour_plane {
        0
    } else {
        sps.chroma_format_idc
    };
    let sps1 = (pcm_luma << 0)
        | (pcm_chroma << 4)
        | (pcm_min << 8)
        | (pcm_max << 12)
        | (chroma << 16)
        | ((sps.amp_enabled as u32) << 18)
        | ((sps.pcm_enabled as u32) << 19)
        | ((sps.scaling_list_enabled as u32) << 20)
        | ((sps.strong_intra_smoothing_enabled as u32) << 21);

    let qp_depth = sps
        .log2_ctb_size
        .checked_sub(pps.diff_cu_qp_delta_depth)
        .expect("RPIVID CU QP delta depth exceeds CTB depth");
    let pps_reg = qp_depth
        | ((pps.cu_qp_delta_enabled as u32) << 4)
        | ((pps.transquant_bypass_enabled as u32) << 5)
        | ((pps.transform_skip_enabled as u32) << 6)
        | ((pps.sign_data_hiding_enabled as u32) << 7)
        | ((((pps.pps_cb_qp_offset + slice.slice_cb_qp_offset) as u32) & 0xff) << 8)
        | ((((pps.pps_cr_qp_offset + slice.slice_cr_qp_offset) as u32) & 0xff) << 16)
        | ((pps.constrained_intra_pred as u32) << 24);

    commands.push(RPI_SPS0, sps0);
    commands.push(RPI_SPS1, sps1);
    commands.push(RPI_PPS, pps_reg);
    commands.push(RPI_SLICESTART, 0);
}

fn slice_register(picture: &FirstPictureSyntax, row: u32, slice_const: u32) -> u32 {
    let sps = picture.sps;
    let ctb_size = 1u32 << sps.log2_ctb_size;
    let width_last = sps.width & (ctb_size - 1);
    let height_last = sps.height & (ctb_size - 1);
    let width = if width_last == 0 {
        ctb_size
    } else {
        width_last
    };
    let height = if row + 1 < picture.ctb_height || height_last == 0 {
        ctb_size
    } else {
        height_last
    };

    slice_const | (width << 17) | (height << 24)
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
    let reconstructed = ((window.base_register_value() as u64) << 6) + window.byte_offset() as u64;
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
        assert!(
            size <= usize::MAX as u64,
            "HEVC MMIO size does not fit in usize"
        );

        Self {
            base: physical_base as usize,
            size: size as usize,
        }
    }

    fn read32(self, offset: usize) -> u32 {
        assert_eq!(
            offset & 0x3,
            0,
            "HEVC register offset is not 32-bit aligned"
        );
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("HEVC register offset overflow");
        assert!(end <= self.size, "HEVC register read exceeds MMIO resource");

        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    fn write32(self, offset: usize, value: u32) {
        assert_eq!(
            offset & 0x3,
            0,
            "HEVC register offset is not 32-bit aligned"
        );
        let end = offset
            .checked_add(core::mem::size_of::<u32>())
            .expect("HEVC register offset overflow");
        assert!(
            end <= self.size,
            "HEVC register write exceeds MMIO resource"
        );

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
