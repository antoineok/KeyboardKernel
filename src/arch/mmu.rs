//! AArch64 EL1 stage-1 MMU setup for the BCM2711.
//!
//! The initial virtual-memory layout is deliberately simple:
//! - 4 KiB translation granule
//! - 39-bit virtual addresses
//! - TTBR0_EL1 only
//! - identity mapping (VA == PA)
//! - 2 MiB L2 blocks whenever possible, with 4 KiB L3 fallbacks
//!
//! Normal RAM is mapped cacheable, shared DMA memory is mapped as normal
//! non-cacheable memory, and the BCM2711 peripheral window is mapped as
//! Device-nGnRE. The first bring-up enables the MMU and instruction cache but
//! intentionally leaves the data cache disabled until shared-memory cache
//! maintenance is implemented for mailbox/DMA users.

#![allow(dead_code)]

use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::memory::{boot::BOOT_ALLOCATOR, MemoryRegionKind, PhysAddr, PhysSize, MEMORY_MANAGER};

/// 4 KiB translation granule.
const PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = !(PAGE_SIZE - 1);

/// 2 MiB block size at translation level 2.
const BLOCK_SIZE_L2: u64 = 2 * 1024 * 1024;
const BLOCK_MASK_L2: u64 = !(BLOCK_SIZE_L2 - 1);

/// Entries in every 4 KiB translation table.
const ENTRIES_PER_TABLE: usize = 512;

/// Identity mapping for the initial kernel address space.
pub const KERNEL_VIRT_OFFSET: u64 = 0;

/// High virtual alias for the BCM2711 firmware secondary-CPU spin table.
///
/// Physical page zero is intentionally kept out of the ordinary identity map
/// so null-pointer faults remain useful. CPU0 uses this alias to write the
/// armstub release slots at physical 0xE0/0xE8/0xF0.
pub const SECONDARY_SPIN_TABLE_VIRT: u64 = 0x0000_007f_ffff_f000;

/// BCM2711 peripheral space used by the drivers in this kernel.
const MMIO_START: u64 = 0xFE00_0000;
const MMIO_END: u64 = 0x1_0000_0000;

/// MAIR_EL1 attribute indices.
const MAIR_IDX_NORMAL: u64 = 0;
const MAIR_IDX_DEVICE: u64 = 1;
const MAIR_IDX_NORMAL_NC: u64 = 2;

/// MAIR attribute encodings.
const MAIR_ATTR_NORMAL_WB_RA_WA: u64 = 0xFF;
const MAIR_ATTR_DEVICE_NGNRE: u64 = 0x04;
const MAIR_ATTR_NORMAL_NC: u64 = 0x44;

/// SCTLR_EL1 controls used during initial bring-up.
const SCTLR_M: u64 = 1 << 0;
const SCTLR_C: u64 = 1 << 2;
const SCTLR_I: u64 = 1 << 12;

/// TCR_EL1 configuration for a 39-bit TTBR0 address space and 4 KiB pages.
const TCR_T0SZ_39_BIT: u64 = 25;
const TCR_IRGN0_WB_RA_WA: u64 = 0b01 << 8;
const TCR_ORGN0_WB_RA_WA: u64 = 0b01 << 10;
const TCR_SH0_INNER: u64 = 0b11 << 12;
const TCR_TG0_4K: u64 = 0b00 << 14;

// TTBR1 is disabled, but keep its size/granule fields architecturally sane.
const TCR_T1SZ_39_BIT: u64 = 25 << 16;
const TCR_EPD1: u64 = 1 << 23;
const TCR_TG1_4K: u64 = 0b10 << 30;

// Cortex-A72 implements a 40-bit physical address space.
const TCR_IPS_40_BIT: u64 = 0b010 << 32;

const TCR_VALUE: u64 = TCR_T0SZ_39_BIT
    | TCR_IRGN0_WB_RA_WA
    | TCR_ORGN0_WB_RA_WA
    | TCR_SH0_INNER
    | TCR_TG0_4K
    | TCR_T1SZ_39_BIT
    | TCR_EPD1
    | TCR_TG1_4K
    | TCR_IPS_40_BIT;

const MAIR_VALUE: u64 = (MAIR_ATTR_NORMAL_WB_RA_WA << (MAIR_IDX_NORMAL * 8))
    | (MAIR_ATTR_DEVICE_NGNRE << (MAIR_IDX_DEVICE * 8))
    | (MAIR_ATTR_NORMAL_NC << (MAIR_IDX_NORMAL_NC * 8));

/// Root table published by CPU0 for secondary CPUs.
///
/// Secondary CPUs start with their local MMU disabled, but the page tables
/// themselves already live in identity-addressable physical RAM. CPU0 stores
/// the root here before any secondary is released.
static KERNEL_L1_PHYS: AtomicU64 = AtomicU64::new(0);

/// Descriptor bits for block, page and table entries.
mod desc {
    pub const VALID: u64 = 1 << 0;

    // Descriptor type bits [1:0].
    pub const BLOCK: u64 = 0b01;
    pub const TABLE: u64 = 0b11;
    pub const PAGE: u64 = 0b11;

    pub const AF: u64 = 1 << 10;

    // Shareability SH[1:0].
    pub const SH_OUTER: u64 = 0b10 << 8;
    pub const SH_INNER: u64 = 0b11 << 8;

    // AP[2:1] = 00: EL1 read/write, EL0 no access.
    pub const AP_RW_EL1: u64 = 0b00 << 6;

    // AttrIndx[2:0].
    pub const ATTRIDX_NORMAL: u64 = super::MAIR_IDX_NORMAL << 2;
    pub const ATTRIDX_DEVICE: u64 = super::MAIR_IDX_DEVICE << 2;
    pub const ATTRIDX_NORMAL_NC: u64 = super::MAIR_IDX_NORMAL_NC << 2;

    pub const UXN: u64 = 1 << 54;
    pub const PXN: u64 = 1 << 53;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MappingKind {
    Normal,
    NormalNonCacheable,
    Device,
}

impl MappingKind {
    #[inline(always)]
    const fn descriptor_bits(self) -> u64 {
        match self {
            Self::Normal => desc::ATTRIDX_NORMAL | desc::SH_INNER,
            Self::NormalNonCacheable => {
                desc::ATTRIDX_NORMAL_NC | desc::SH_INNER | desc::UXN | desc::PXN
            }
            Self::Device => desc::ATTRIDX_DEVICE | desc::SH_OUTER | desc::UXN | desc::PXN,
        }
    }
}

/// Three-level translation hierarchy rooted at an L1 table.
///
/// With a 39-bit address space and 4 KiB granule:
/// - VA[38:30] selects L1
/// - VA[29:21] selects L2
/// - VA[20:12] selects L3
pub struct Mmu {
    l1_phys: PhysAddr,
}

impl Mmu {
    /// Allocate and clear the root L1 table from the early boot allocator.
    pub unsafe fn new() -> Self {
        let l1_phys = BOOT_ALLOCATOR
            .get_mut()
            .alloc(PAGE_SIZE, PAGE_SIZE)
            .expect("BOOT_ALLOCATOR: out of memory for L1 table");

        zero_table(l1_phys);

        Self { l1_phys }
    }

    #[inline(always)]
    pub const fn l1_phys(&self) -> PhysAddr {
        self.l1_phys
    }

    /// Map a physical range as normal write-back RAM.
    pub unsafe fn map_normal_identity(&mut self, phys: PhysAddr, size: PhysSize) {
        self.map_range(phys, size, MappingKind::Normal);
    }

    /// Map a physical range as normal non-cacheable shared/DMA memory.
    pub unsafe fn map_normal_nocache_identity(&mut self, phys: PhysAddr, size: PhysSize) {
        self.map_range(phys, size, MappingKind::NormalNonCacheable);
    }

    /// Map a physical range as Device-nGnRE MMIO.
    pub unsafe fn map_device_identity(&mut self, phys: PhysAddr, size: PhysSize) {
        self.map_range(phys, size, MappingKind::Device);
    }

    /// Enable the EL1 MMU using this translation table.
    ///
    /// The data cache intentionally remains disabled. This keeps the existing
    /// mailbox, framebuffer and DMA behaviour coherent during the first MMU
    /// bring-up. A later cache-specific change can enable SCTLR_EL1.C once all
    /// shared-memory users have explicit cache maintenance or uncached aliases.
    pub unsafe fn enable(&self) {
        // Publish the root before CPU0 can later release any secondary CPU.
        KERNEL_L1_PHYS.store(self.l1_phys, Ordering::Release);
        enable_translation_root(self.l1_phys);
    }

    unsafe fn map_range(&mut self, start: PhysAddr, size: PhysSize, kind: MappingKind) {
        if size == 0 {
            return;
        }

        let raw_end = start.checked_add(size).expect("MMU mapping range overflow");
        let mut addr = align_down(start, PAGE_SIZE);
        let end = align_up(raw_end, PAGE_SIZE).expect("MMU mapping alignment overflow");

        while addr < end {
            let va = addr
                .checked_add(KERNEL_VIRT_OFFSET)
                .expect("MMU virtual address overflow");

            let can_block = (addr & (BLOCK_SIZE_L2 - 1)) == 0
                && (va & (BLOCK_SIZE_L2 - 1)) == 0
                && (end - addr) >= BLOCK_SIZE_L2;

            if can_block {
                self.map_block_l2(va, addr, kind);
                addr = addr
                    .checked_add(BLOCK_SIZE_L2)
                    .expect("MMU block mapping overflow");
            } else {
                self.map_page_l3(va, addr, kind);
                addr = addr
                    .checked_add(PAGE_SIZE)
                    .expect("MMU page mapping overflow");
            }
        }
    }

    /// Remove identity mappings for every page touched by a `NoMap` range.
    ///
    /// Memory-map boundaries are byte-granular, while stage-1 mappings are
    /// page-granular. Rounding `NoMap` outward is the conservative choice: a
    /// page containing even one byte of a no-map reservation stays invalid.
    unsafe fn unmap_identity(&mut self, start: PhysAddr, size: PhysSize) {
        if size == 0 {
            return;
        }

        let raw_end = start.checked_add(size).expect("MMU unmap range overflow");
        let mut addr = align_down(start, PAGE_SIZE);
        let end = align_up(raw_end, PAGE_SIZE).expect("MMU unmap alignment overflow");

        while addr < end {
            let va = addr
                .checked_add(KERNEL_VIRT_OFFSET)
                .expect("MMU unmap virtual address overflow");

            self.unmap_page_l3(va);
            addr = addr
                .checked_add(PAGE_SIZE)
                .expect("MMU unmap page overflow");
        }
    }

    unsafe fn unmap_page_l3(&mut self, va: u64) {
        let l1_idx = ((va >> 30) & 0x1FF) as usize;
        let l2_idx = ((va >> 21) & 0x1FF) as usize;
        let l3_idx = ((va >> 12) & 0x1FF) as usize;

        let l1 = self.l1_phys as *mut u64;
        let l1_value = ptr::read_volatile(l1.add(l1_idx));

        if l1_value & desc::VALID == 0 {
            return;
        }

        assert_eq!(
            l1_value & 0b11,
            desc::TABLE,
            "MMU NoMap range collides with an L1 block descriptor"
        );

        let l2 = (l1_value & PAGE_MASK) as *mut u64;
        let l2_value = ptr::read_volatile(l2.add(l2_idx));

        if l2_value & desc::VALID == 0 {
            return;
        }

        assert_eq!(
            l2_value & 0b11,
            desc::TABLE,
            "MMU NoMap range collides with an L2 block descriptor"
        );

        let l3 = (l2_value & PAGE_MASK) as *mut u64;
        ptr::write_volatile(l3.add(l3_idx), 0);
    }

    unsafe fn map_block_l2(&mut self, va: u64, pa: u64, kind: MappingKind) {
        let l1_idx = ((va >> 30) & 0x1FF) as usize;
        let l2_idx = ((va >> 21) & 0x1FF) as usize;

        let l1 = self.l1_phys as *mut u64;
        let l2 = self.next_level_table(l1, l1_idx);
        let entry = l2.add(l2_idx);

        let descriptor = (pa & BLOCK_MASK_L2)
            | desc::VALID
            | desc::BLOCK
            | desc::AF
            | desc::AP_RW_EL1
            | kind.descriptor_bits();

        ptr::write_volatile(entry, descriptor);
    }

    unsafe fn map_page_l3(&mut self, va: u64, pa: u64, kind: MappingKind) {
        let l1_idx = ((va >> 30) & 0x1FF) as usize;
        let l2_idx = ((va >> 21) & 0x1FF) as usize;
        let l3_idx = ((va >> 12) & 0x1FF) as usize;

        let l1 = self.l1_phys as *mut u64;
        let l2 = self.next_level_table(l1, l1_idx);
        let l3 = self.next_level_table(l2, l2_idx);
        let entry = l3.add(l3_idx);

        let descriptor = (pa & PAGE_MASK)
            | desc::VALID
            | desc::PAGE
            | desc::AF
            | desc::AP_RW_EL1
            | kind.descriptor_bits();

        ptr::write_volatile(entry, descriptor);
    }

    /// Get or allocate the next-level table referenced by `table[index]`.
    unsafe fn next_level_table(&mut self, table: *mut u64, index: usize) -> *mut u64 {
        let entry = table.add(index);
        let value = ptr::read_volatile(entry);

        if value & desc::VALID != 0 {
            // This helper is only used at levels where the caller requires a
            // table descriptor. A block descriptor here would indicate
            // overlapping/incompatible mappings in the memory map.
            assert_eq!(
                value & 0b11,
                desc::TABLE,
                "MMU mapping collides with an existing block descriptor"
            );

            return (value & PAGE_MASK) as *mut u64;
        }

        let next_phys = BOOT_ALLOCATOR
            .get_mut()
            .alloc(PAGE_SIZE, PAGE_SIZE)
            .expect("BOOT_ALLOCATOR: out of memory for page table");

        zero_table(next_phys);

        let descriptor = (next_phys & PAGE_MASK) | desc::VALID | desc::TABLE;
        ptr::write_volatile(entry, descriptor);

        next_phys as *mut u64
    }
}

/// Install CPU0's already-built kernel address space on the current CPU.
///
/// Returns `false` if CPU0 has not published a root table yet. No allocation
/// or mutable access to the page tables occurs here.
pub unsafe fn enable_published_kernel_mmu() -> bool {
    let l1_phys = KERNEL_L1_PHYS.load(Ordering::Acquire);

    if l1_phys == 0 {
        return false;
    }

    enable_translation_root(l1_phys);
    true
}

unsafe fn enable_translation_root(l1_phys: PhysAddr) {
    // Ensure all page-table writes performed by CPU0 are globally visible
    // before this CPU's translation-table walker begins using them.
    asm!("dsb sy", options(nostack, preserves_flags));

    asm!(
        "msr mair_el1, {mair}",
        "msr tcr_el1, {tcr}",
        "msr ttbr0_el1, {ttbr0}",
        "msr ttbr1_el1, xzr",
        mair = in(reg) MAIR_VALUE,
        tcr = in(reg) TCR_VALUE,
        ttbr0 = in(reg) l1_phys,
        options(nostack, preserves_flags),
    );

    asm!(
        "isb",
        "tlbi vmalle1",
        "dsb sy",
        "isb",
        "ic iallu",
        "dsb sy",
        "isb",
        options(nostack, preserves_flags),
    );

    let mut sctlr: u64;
    asm!(
        "mrs {value}, sctlr_el1",
        value = out(reg) sctlr,
        options(nomem, nostack, preserves_flags),
    );

    // Keep D-cache disabled until shared-memory/cache-coherency handling is
    // explicitly completed for every DMA user.
    sctlr |= SCTLR_M | SCTLR_I;
    sctlr &= !SCTLR_C;

    asm!(
        "msr sctlr_el1, {value}",
        "isb",
        value = in(reg) sctlr,
        options(nostack, preserves_flags),
    );
}

/// Return whether EL1 stage-1 translation is currently enabled.
#[inline(always)]
pub fn is_enabled() -> bool {
    let sctlr: u64;

    unsafe {
        asm!(
            "mrs {value}, sctlr_el1",
            value = out(reg) sctlr,
            options(nomem, nostack, preserves_flags),
        );
    }

    sctlr & SCTLR_M != 0
}

/// Build the initial identity-mapped EL1 address space.
///
/// Memory-map semantics are deliberately different from allocator semantics:
/// `Reserved` RAM still has to be mapped because it contains the kernel,
/// stacks, boot allocator, firmware-owned RAM, and page tables. `NoMap` is the
/// only RAM classification intentionally omitted from the translation tables.
pub unsafe fn map_kernel_and_mmio() -> Mmu {
    let mut mmu = Mmu::new();
    let memory_map = MEMORY_MANAGER.get().map();

    // First establish ordinary RAM mappings. Runtime reservations such as the
    // kernel image and boot allocator remain accessible even though the page
    // allocator must not hand them out.
    for region in memory_map.iter() {
        if matches!(
            region.kind,
            MemoryRegionKind::Usable | MemoryRegionKind::Reserved
        ) {
            mmu.map_normal_identity(region.start, region.size);
        }
    }

    // Shared/DMA memory has higher mapping priority than ordinary RAM. Mapping
    // it in a second pass also makes page-rounded boundary mappings safely
    // non-cacheable.
    for region in memory_map.iter() {
        if region.kind == MemoryRegionKind::Dma {
            mmu.map_normal_nocache_identity(region.start, region.size);
        }
    }

    // `NoMap` has the highest priority. Clear every page it touches after the
    // page-rounded mappings above have been established.
    for region in memory_map.iter() {
        if region.kind == MemoryRegionKind::NoMap {
            mmu.unmap_identity(region.start, region.size);
        }
    }

    // The Raspberry Pi armstub keeps secondary-core release slots in physical
    // page zero. Map that page only through a high Normal-NC alias instead of
    // identity-mapping VA 0, preserving a useful null-pointer guard.
    mmu.map_page_l3(
        SECONDARY_SPIN_TABLE_VIRT,
        0,
        MappingKind::NormalNonCacheable,
    );

    mmu.map_device_identity(MMIO_START, MMIO_END - MMIO_START);
    mmu
}

#[inline(always)]
const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

#[inline(always)]
const fn align_up(value: u64, alignment: u64) -> Option<u64> {
    let mask = alignment - 1;
    match value.checked_add(mask) {
        Some(aligned) => Some(aligned & !mask),
        None => None,
    }
}

unsafe fn zero_table(phys: PhysAddr) {
    let table = phys as *mut u64;

    for index in 0..ENTRIES_PER_TABLE {
        ptr::write_volatile(table.add(index), 0);
    }
}
