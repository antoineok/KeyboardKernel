// src/arch/mmu.rs

//! AArch64 MMU & page-table setup for EL1.
//!
//! This version:
//! - Uses 4 KiB granule
//! - Uses a 3-level scheme (L1 root, then L2, L3)
//! - Identity-maps RAM and MMIO
//! - Uses 2 MiB L2 block mappings whenever possible
//!   (dramatically reduces page-table memory usage)
//!
//! Higher-half can be added later by changing KERNEL_VIRT_OFFSET
//! and adding extra mappings for the kernel region.

#![allow(dead_code)]

use core::ptr;

use crate::memory::{boot::BOOT_ALLOCATOR, MemoryRegionKind, PhysAddr, PhysSize, MEMORY_MANAGER};

/// 4 KiB page size.
const PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = !(PAGE_SIZE - 1);

/// 2 MiB block size at L2.
const BLOCK_SIZE_L2: u64 = 2 * 1024 * 1024;
const BLOCK_MASK_L2: u64 = !(BLOCK_SIZE_L2 - 1);

/// Entries per page-table (9 bits → 512).
const ENTRIES_PER_TABLE: usize = 512;

/// For now, identity mapping (VA == PA).
/// Later you can set this to a higher-half base and
/// add a mapping just for the kernel.
pub const KERNEL_VIRT_OFFSET: u64 = 0;

/// MAIR index for normal and device memory.
const MAIR_IDX_NORMAL: u64 = 0;
const MAIR_IDX_DEVICE: u64 = 1;

/// Descriptor bits for page and table entries.
mod desc {
    // Common descriptor bits
    pub const VALID: u64 = 1 << 0;

    // Type bits [1:0]
    pub const BLOCK: u64 = 0b01; // L1/L2 block
    pub const TABLE: u64 = 0b11; // table pointer
    pub const PAGE: u64 = 0b11; // L3 page

    // Access flag
    pub const AF: u64 = 1 << 10;

    // Shareability (SH[1:0])
    pub const SH_INNER: u64 = 0b11 << 8; // inner-shareable

    // Access permissions (AP[2:1])
    // 0b00 = EL1 RW, EL0 none
    pub const AP_RW_EL1: u64 = 0b00 << 6;

    // AttrIndx[2:0]
    pub const ATTRIDX_NORMAL: u64 = super::MAIR_IDX_NORMAL << 2;
    pub const ATTRIDX_DEVICE: u64 = super::MAIR_IDX_DEVICE << 2;

    // Execute-never bits
    pub const UXN: u64 = 1 << 54;
    pub const PXN: u64 = 1 << 53;
}

/// 3-level page-table hierarchy (L1 root).
///
/// VA bits:
/// - [38:30] → L1 index
/// - [29:21] → L2 index
/// - [20:12] → L3 index
pub struct Mmu {
    /// Physical address of the L1 (root) table.
    l1_phys: PhysAddr,
}

impl Mmu {
    /// Allocate and zero the L1 root table.
    pub unsafe fn new() -> Self {
        let l1_phys = BOOT_ALLOCATOR
            .get_mut()
            .alloc(PAGE_SIZE, PAGE_SIZE)
            .expect("BOOT_ALLOCATOR: out of memory for L1 table");

        let l1_ptr = l1_phys as *mut u64;
        for i in 0..ENTRIES_PER_TABLE {
            ptr::write_volatile(l1_ptr.add(i), 0);
        }

        Self { l1_phys }
    }

    pub fn l1_phys(&self) -> PhysAddr {
        self.l1_phys
    }

    /// Map [phys, phys+size) as **normal cacheable** memory,
    /// identity-mapped (VA = phys + KERNEL_VIRT_OFFSET).
    pub unsafe fn map_normal_identity(&mut self, phys: PhysAddr, size: PhysSize) {
        self.map_range(phys, phys + size, false);
    }

    /// Map [phys, phys+size) as **device** memory (MMIO),
    /// identity-mapped.
    pub unsafe fn map_device_identity(&mut self, phys: PhysAddr, size: PhysSize) {
        self.map_range(phys, phys + size, true);
    }

    /// Map [start, end) using 2 MiB L2 blocks when possible,
    /// falling back to 4 KiB pages when necessary.
    unsafe fn map_range(&mut self, start: PhysAddr, end: PhysAddr, device: bool) {
        let mut addr = start;
        let end = end;

        while addr < end {
            let va = addr + KERNEL_VIRT_OFFSET;

            let can_block = (addr & (BLOCK_SIZE_L2 - 1)) == 0
                && (va & (BLOCK_SIZE_L2 - 1)) == 0
                && (end - addr) >= BLOCK_SIZE_L2;

            if can_block {
                self.map_block_l2(va, addr, device);
                addr += BLOCK_SIZE_L2;
            } else {
                self.map_page_l3(va, addr, device);
                addr += PAGE_SIZE;
            }
        }
    }

    /// Map a 2 MiB block at L2.
    unsafe fn map_block_l2(&mut self, va: u64, pa: u64, device: bool) {
        let l1_idx = ((va >> 30) & 0x1FF) as usize;
        let l2_idx = ((va >> 21) & 0x1FF) as usize;

        let l1 = self.l1_phys as *mut u64;
        let l2 = self.next_level_table(l1, l1_idx);

        let entry = l2.add(l2_idx);

        let attridx = if device {
            desc::ATTRIDX_DEVICE
        } else {
            desc::ATTRIDX_NORMAL
        };

        // For normal memory, keep executable (no PXN/UXN).
        // For device, force XN.
        let xn = if device { desc::UXN | desc::PXN } else { 0 };

        let desc = (pa & BLOCK_MASK_L2)
            | desc::VALID
            | desc::BLOCK
            | desc::AF
            | desc::SH_INNER
            | desc::AP_RW_EL1
            | attridx
            | xn;

        ptr::write_volatile(entry, desc);
    }

    /// Map a 4 KiB page at L3.
    unsafe fn map_page_l3(&mut self, va: u64, pa: u64, device: bool) {
        let l1_idx = ((va >> 30) & 0x1FF) as usize;
        let l2_idx = ((va >> 21) & 0x1FF) as usize;
        let l3_idx = ((va >> 12) & 0x1FF) as usize;

        let l1 = self.l1_phys as *mut u64;
        let l2 = self.next_level_table(l1, l1_idx);
        let l3 = self.next_level_table(l2, l2_idx);

        let entry = l3.add(l3_idx);

        let attridx = if device {
            desc::ATTRIDX_DEVICE
        } else {
            desc::ATTRIDX_NORMAL
        };

        // For normal memory, keep executable (no PXN/UXN).
        // For device, force XN.
        let xn = if device { desc::UXN | desc::PXN } else { 0 };

        let desc = (pa & PAGE_MASK)
            | desc::VALID
            | desc::PAGE
            | desc::AF
            | desc::SH_INNER
            | desc::AP_RW_EL1
            | attridx
            | xn;

        ptr::write_volatile(entry, desc);
    }

    /// Get or allocate the next-level table from parent `table[index]`.
    /// Used for:
    /// - L1 → L2
    /// - L2 → L3
    unsafe fn next_level_table(&mut self, table: *mut u64, index: usize) -> *mut u64 {
        let entry = table.add(index);
        let val = ptr::read_volatile(entry);

        if val & desc::VALID != 0 {
            // Existing table descriptor.
            let next_pa = val & PAGE_MASK;
            next_pa as *mut u64
        } else {
            // Allocate a new table page.
            let next_pa = BOOT_ALLOCATOR
                .get_mut()
                .alloc(PAGE_SIZE, PAGE_SIZE)
                .expect("BOOT_ALLOCATOR: out of memory for page table");
            let next_ptr = next_pa as *mut u64;

            // Zero it.
            for i in 0..ENTRIES_PER_TABLE {
                ptr::write_volatile(next_ptr.add(i), 0);
            }

            // Install table descriptor.
            let desc = (next_pa & PAGE_MASK) | desc::VALID | desc::TABLE;
            ptr::write_volatile(entry, desc);

            next_ptr
        }
    }
}

/// High-level helper: map kernel RAM + MMIO with identity mappings.
///
/// - All `Usable` regions from the memory map → normal memory
/// - A basic FE00_0000..FE20_0000 range → device memory (UART, GPIO, etc.)
pub unsafe fn map_kernel_and_mmio() -> Mmu {
    let mut mmu = Mmu::new();

    let mm = MEMORY_MANAGER.get().map();

    // Map all usable RAM as normal memory.
    for region in mm.iter() {
        if region.kind == MemoryRegionKind::Usable {
            mmu.map_normal_identity(region.start, region.size);
        }
    }

    // Map a basic MMIO window in the FE... range as device memory.
    // You can refine this later based on FDT MMIO entries.
    let mmio_start: u64 = 0xFE00_0000;
    let mmio_size: u64 = 0x0200_0000; // 32 MiB window
    mmu.map_device_identity(mmio_start, mmio_size);

    mmu
}
