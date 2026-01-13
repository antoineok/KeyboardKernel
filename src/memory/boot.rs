// src/memory/boot.rs

//! Simple early boot-time physical allocator (bump allocator).
//!
//! Intended for:
//! - Page tables
//! - Early kernel data structures
//! - Temporary buffers
//!
//! Must only be used during early boot, before a real page allocator exists.

use super::{PhysAddr, PhysSize};

pub struct BootAllocator {
    current: PhysAddr,
    limit: PhysAddr,
    initialized: bool,
}

impl BootAllocator {
    /// Creates an empty, uninitialized allocator.
    pub const fn new() -> Self {
        Self {
            current: 0,
            limit: 0,
            initialized: false,
        }
    }

    /// Initialize the allocator with a physical range [start, end).
    ///
    /// You decide where this range comes from:
    /// - from the first usable region after the kernel image
    /// - or from a dedicated area (e.g. after __heap_end)
    pub fn init(&mut self, start: PhysAddr, end: PhysAddr) {
        assert!(start < end, "BootAllocator: invalid range");

        // Small alignment for the base (16 bytes is enough here; you may want 4K).
        let aligned_start = Self::align_up(start, 16);

        self.current = aligned_start;
        self.limit = end;
        self.initialized = true;
    }

    /// Allocate `size` bytes with `align` alignment.
    ///
    /// Returns physical address on success, or None if out of space.
    pub fn alloc(&mut self, size: PhysSize, align: PhysSize) -> Option<PhysAddr> {
        assert!(
            self.initialized,
            "BootAllocator used before init() was called"
        );

        let align = align.max(1);
        let size = size.max(1);

        let start = Self::align_up(self.current, align);
        let end = match start.checked_add(size) {
            Some(e) => e,
            None => return None,
        };

        if end > self.limit {
            return None;
        }

        self.current = end;
        Some(start)
    }

    /// How many bytes remain in this boot allocation range.
    pub fn remaining(&self) -> PhysSize {
        if !self.initialized {
            return 0;
        }
        if self.limit > self.current {
            self.limit - self.current
        } else {
            0
        }
    }

    /// Return the current usable subrange [current, limit), if initialized.
    pub fn range(&self) -> Option<(PhysAddr, PhysAddr)> {
        if self.initialized {
            Some((self.current, self.limit))
        } else {
            None
        }
    }

    #[inline]
    fn align_up(addr: PhysAddr, align: PhysAddr) -> PhysAddr {
        let mask = align - 1;
        (addr + mask) & !mask
    }
}

/// Global boot allocator instance.
///
/// Typical usage in your init code:
///
/// ```ignore
/// use crate::memory::{MEMORY_MANAGER, MemoryRegionKind};
/// use crate::memory::boot::BOOT_ALLOCATOR;
///
/// unsafe {
///     // Example: choose a range starting after __heap_end,
///     // inside the first Usable region.
///     extern "C" {
///         static __heap_end: u8;
///     }
///
///     let base = &__heap_end as *const u8 as u64;
///     let mm = MEMORY_MANAGER.map();
///
///     let mut limit = None;
///     for region in mm.iter() {
///         if region.kind == MemoryRegionKind::Usable
///             && base >= region.start
///             && base < region.end()
///         {
///             limit = Some(region.end());
///             break;
///         }
///     }
///
///     let limit = limit.expect("No usable region for boot allocator");
///     BOOT_ALLOCATOR.init(base, limit);
///
///     let pt_phys = BOOT_ALLOCATOR
///         .alloc(0x1000, 0x1000)
///         .expect("boot alloc failed");
/// }
/// ```
pub static mut BOOT_ALLOCATOR: BootAllocator = BootAllocator::new();
