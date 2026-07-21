// src/memory/boot.rs

//! Simple early boot-time physical allocator (bump allocator).
//!
//! Intended for:
//! - Page tables
//! - Early kernel data structures
//! - Temporary buffers
//!
//! Must only be used during early boot, before a real page allocator exists.

use core::cell::UnsafeCell;

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
pub struct BootStatic(UnsafeCell<BootAllocator>);

unsafe impl Sync for BootStatic {}

pub static BOOT_ALLOCATOR: BootStatic = BootStatic(UnsafeCell::new(BootAllocator::new()));

impl BootStatic {
    pub fn get_mut(&self) -> &mut BootAllocator {
        unsafe { &mut *self.0.get() }
    }

    pub fn get(&self) -> &BootAllocator {
        unsafe { &*self.0.get() }
    }
}