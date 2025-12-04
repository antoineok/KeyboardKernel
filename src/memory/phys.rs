// src/memory/phys.rs

//! Physical page-frame allocator (bitmap-based).
//!
//! - Uses 4 KiB pages.
//! - Builds on top of `MemoryMap` from `memory::mod`.
//! - Uses the boot allocator to allocate its own bitmap.
//!
//! Convention:
//! - One bit per page.
//! - Bit = 0 → free
//! - Bit = 1 → allocated / reserved

use core::cmp::{max, min};

use super::boot::BOOT_ALLOCATOR;
use super::{MemoryMap, MemoryRegionKind, PhysAddr, PhysSize};

const PAGE_SIZE: PhysSize = 4096;

pub struct PhysAllocator {
    arena_start: PhysAddr,
    arena_end: PhysAddr,
    page_size: PhysSize,

    /// Pointer to a bitmap in (identity-mapped) RAM.
    /// Each bit corresponds to one page.
    bitmap: *mut u64,
    /// Number of u64 entries in the bitmap.
    bitmap_len: usize,

    /// Total number of pages in the arena.
    total_pages: usize,

    initialized: bool,
}

impl PhysAllocator {
    pub const fn new() -> Self {
        Self {
            arena_start: 0,
            arena_end: 0,
            page_size: PAGE_SIZE,
            bitmap: core::ptr::null_mut(),
            bitmap_len: 0,
            total_pages: 0,
            initialized: false,
        }
    }

    /// Initialize the allocator using the global MemoryMap.
    pub unsafe fn init_from_memory_map(&mut self, mm: &MemoryMap) {
        assert!(!self.initialized, "PhysAllocator already initialized");

        // 1) Find the global arena [arena_start, arena_end) over all Usable regions.
        let mut start: Option<PhysAddr> = None;
        let mut end: Option<PhysAddr> = None;

        for region in mm.iter() {
            if region.kind == MemoryRegionKind::Usable {
                let r_start = region.start;
                let r_end = region.end();

                start = Some(match start {
                    Some(s) => s.min(r_start),
                    None => r_start,
                });
                end = Some(match end {
                    Some(e) => e.max(r_end),
                    None => r_end,
                });
            }
        }

        let arena_start = start.expect("PhysAllocator: no Usable memory regions");
        let arena_end = end.unwrap();

        let total_bytes = arena_end - arena_start;
        let total_pages = (total_bytes + (PAGE_SIZE - 1)) / PAGE_SIZE;

        // 2) Allocate bitmap: one bit per page.
        let bitmap_bits = total_pages as usize;
        let bitmap_u64s = (bitmap_bits + 63) / 64;
        let bitmap_bytes = (bitmap_u64s * core::mem::size_of::<u64>()) as u64;

        let bitmap_phys = BOOT_ALLOCATOR
            .alloc(bitmap_bytes, 8)
            .expect("PhysAllocator: failed to allocate bitmap from boot allocator");

        // Identity map: phys == virt (for now).
        let bitmap_ptr = bitmap_phys as *mut u64;

        // 3) Initialize bitmap: everything allocated (1) by default.
        for i in 0..bitmap_u64s {
            core::ptr::write_volatile(bitmap_ptr.add(i), u64::MAX);
        }

        self.arena_start = arena_start;
        self.arena_end = arena_end;
        self.bitmap = bitmap_ptr;
        self.bitmap_len = bitmap_u64s;
        self.total_pages = total_pages as usize;
        self.initialized = true;

        // 4) Mark all Usable regions as free.
        for region in mm.iter() {
            if region.kind == MemoryRegionKind::Usable {
                self.mark_range(region.start, region.end(), true);
            }
        }

        // 5) Mark Reserved / NoMap / Dma as allocated again.
        for region in mm.iter() {
            match region.kind {
                MemoryRegionKind::Reserved | MemoryRegionKind::NoMap | MemoryRegionKind::Dma => {
                    self.mark_range(region.start, region.end(), false);
                }
                MemoryRegionKind::Usable => {}
            }
        }
    }

    /// Allocate a single 4 KiB page.
    pub unsafe fn alloc_page(&mut self) -> Option<PhysAddr> {
        assert!(self.initialized, "PhysAllocator used before init");

        for word_index in 0..self.bitmap_len {
            let word = core::ptr::read_volatile(self.bitmap.add(word_index));

            if word == u64::MAX {
                continue;
            }

            let inv = !word;
            let bit_pos = inv.trailing_zeros() as usize;
            let mask = 1u64 << bit_pos;

            let new_word = word | mask;
            core::ptr::write_volatile(self.bitmap.add(word_index), new_word);

            let page_index = word_index * 64 + bit_pos;
            if page_index >= self.total_pages {
                return None;
            }

            let page_addr = self.arena_start + (page_index as u64) * self.page_size;

            if page_addr >= self.arena_end {
                return None;
            }

            return Some(page_addr);
        }

        None
    }

    /// Allocate `count` contiguous 4 KiB pages.
    ///
    /// Returns the physical address of the first page, or None.
    pub unsafe fn alloc_contiguous(&mut self, count: usize) -> Option<PhysAddr> {
        assert!(self.initialized, "PhysAllocator used before init");
        if count == 0 {
            return None;
        }

        let max_start = self.total_pages.saturating_sub(count);

        'outer: for start_page in 0..=max_start {
            let mut all_free = true;

            for offset in 0..count {
                let page_index = start_page + offset;
                let word_index = page_index / 64;
                let bit_pos = page_index % 64;

                if word_index >= self.bitmap_len {
                    all_free = false;
                    break;
                }

                let word = core::ptr::read_volatile(self.bitmap.add(word_index));
                if (word & (1u64 << bit_pos)) != 0 {
                    all_free = false;
                    break;
                }
            }

            if !all_free {
                continue 'outer;
            }

            // Mark them allocated.
            for offset in 0..count {
                let page_index = start_page + offset;
                let word_index = page_index / 64;
                let bit_pos = page_index % 64;

                let word = core::ptr::read_volatile(self.bitmap.add(word_index));
                let mask = 1u64 << bit_pos;
                let new_word = word | mask;
                core::ptr::write_volatile(self.bitmap.add(word_index), new_word);
            }

            let addr = self.arena_start + (start_page as u64) * self.page_size;
            if addr + (count as u64) * self.page_size > self.arena_end {
                return None;
            }

            return Some(addr);
        }

        None
    }

    /// Free a single 4 KiB page.
    pub unsafe fn free_page(&mut self, pa: PhysAddr) {
        assert!(self.initialized, "PhysAllocator used before init");

        if pa < self.arena_start || pa >= self.arena_end {
            return;
        }

        let page_index = ((pa - self.arena_start) / self.page_size) as usize;
        let word_index = page_index / 64;
        let bit_pos = page_index % 64;

        if word_index >= self.bitmap_len {
            return;
        }

        let word = core::ptr::read_volatile(self.bitmap.add(word_index));
        let mask = !(1u64 << bit_pos);
        let new_word = word & mask;
        core::ptr::write_volatile(self.bitmap.add(word_index), new_word);
    }

    pub fn info(&self) -> Option<(PhysAddr, PhysAddr, PhysSize)> {
        if !self.initialized {
            return None;
        }
        let bytes = self.arena_end - self.arena_start;
        Some((self.arena_start, self.arena_end, bytes))
    }

    fn mark_range(&mut self, start: PhysAddr, end: PhysAddr, free: bool) {
        if !self.initialized {
            return;
        }

        let s = max(start, self.arena_start);
        let e = min(end, self.arena_end);
        if s >= e {
            return;
        }

        let first_page = ((s - self.arena_start) / self.page_size) as usize;
        let last_page_excl =
            ((e - self.arena_start + self.page_size - 1) / self.page_size) as usize;

        for page_index in first_page..last_page_excl {
            let word_index = page_index / 64;
            let bit_pos = page_index % 64;

            if word_index >= self.bitmap_len {
                break;
            }

            unsafe {
                let word = core::ptr::read_volatile(self.bitmap.add(word_index));
                let mask = 1u64 << bit_pos;

                let new_word = if free {
                    word & !mask // 0 → free
                } else {
                    word | mask // 1 → allocated
                };

                core::ptr::write_volatile(self.bitmap.add(word_index), new_word);
            }
        }
    }
}

pub static mut PHYS_ALLOCATOR: PhysAllocator = PhysAllocator::new();
