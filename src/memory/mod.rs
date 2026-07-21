// src/memory/mod.rs
#![allow(dead_code)]

pub mod boot;
pub mod heap;
pub mod phys;

use crate::fdt::{
    MemoryKind as BootMemoryKind,
    MemoryLayout as BootMemoryLayout,
};

/// Physical address / size types.
pub type PhysAddr = u64;
pub type PhysSize = u64;

/// High-level classification of regions we care about.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MemoryRegionKind {
    /// General-purpose RAM usable for allocations.
    Usable,

    /// Reserved for firmware, the kernel, stacks, or other special purposes.
    Reserved,

    /// Reserved for DMA / CMA pools.
    Dma,

    /// Must not be mapped in the MMU (`no-map`).
    NoMap,
}

/// A single physical-memory region in our internal representation.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct MemoryRegion {
    pub start: PhysAddr,
    pub size: PhysSize,
    pub kind: MemoryRegionKind,
}

impl MemoryRegion {
    #[inline(always)]
    pub const fn end(&self) -> PhysAddr {
        self.start.saturating_add(self.size)
    }

    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    #[inline(always)]
    pub const fn contains(&self, addr: PhysAddr) -> bool {
        addr >= self.start && addr < self.end()
    }

    #[inline(always)]
    pub const fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MemoryMapError {
    CapacityExceeded,
}

pub type MemoryMapResult<T> = core::result::Result<T, MemoryMapError>;

/// Fixed-size, non-overlapping physical-memory map.
///
/// The map is kept sorted after `normalize()` or `overlay()`. Runtime
/// reservations replace the classification of any overlapping region, so a
/// range can never remain both `Usable` and `Reserved`.
pub struct MemoryMap {
    regions: [Option<MemoryRegion>; Self::MAX_REGIONS],
    len: usize,
}

impl MemoryMap {
    /*
     * The boot parser can produce substantially more than 32 entries after
     * subtracting fixed reservations from RAM. Keep enough headroom for
     * runtime reservations that split usable regions further.
     */
    pub const MAX_REGIONS: usize = 128;

    pub const fn new() -> Self {
        Self {
            regions: [None; Self::MAX_REGIONS],
            len: 0,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.regions = [None; Self::MAX_REGIONS];
        self.len = 0;
    }

    /// Append a region without changing existing classifications.
    ///
    /// Prefer `overlay()` for reservations. `add()` is primarily useful while
    /// importing the parser's already non-overlapping usable regions.
    pub fn add(&mut self, region: MemoryRegion) {
        self.try_add(region)
            .expect("Physical memory map capacity exceeded");
    }

    pub fn try_add(&mut self, region: MemoryRegion) -> MemoryMapResult<()> {
        if region.is_empty() {
            return Ok(());
        }

        if self.len == Self::MAX_REGIONS {
            return Err(MemoryMapError::CapacityExceeded);
        }

        self.regions[self.len] = Some(region);
        self.len += 1;
        Ok(())
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions[..self.len]
            .iter()
            .map(|region| region.as_ref().unwrap())
    }

    /// Replace the classification of `[region.start, region.end())`.
    ///
    /// Any existing region intersecting the new one is split as needed. The
    /// new region then occupies the entire requested interval. Consequently,
    /// runtime reservations cannot overlap a remaining `Usable` entry.
    pub fn overlay(&mut self, region: MemoryRegion) {
        self.try_overlay(region)
            .expect("Physical memory map capacity exceeded while overlaying region");
    }

    pub fn try_overlay(&mut self, region: MemoryRegion) -> MemoryMapResult<()> {
        if region.is_empty() {
            return Ok(());
        }

        let overlay_start = region.start;
        let overlay_end = region.end();

        if overlay_end <= overlay_start {
            return Ok(());
        }

        let mut output = Self::new();

        for existing in self.iter().copied() {
            let existing_end = existing.end();

            // No overlap.
            if existing_end <= overlay_start || existing.start >= overlay_end {
                output.try_add(existing)?;
                continue;
            }

            // Preserve the section before the overlaid interval.
            if existing.start < overlay_start {
                output.try_add(MemoryRegion {
                    start: existing.start,
                    size: overlay_start - existing.start,
                    kind: existing.kind,
                })?;
            }

            // Preserve the section after the overlaid interval.
            if existing_end > overlay_end {
                output.try_add(MemoryRegion {
                    start: overlay_end,
                    size: existing_end - overlay_end,
                    kind: existing.kind,
                })?;
            }
        }

        output.try_add(region)?;
        output.normalize();
        *self = output;
        Ok(())
    }

    /// Sort regions and merge touching or overlapping regions of the same kind.
    ///
    /// Different kinds are expected not to overlap. `overlay()` guarantees
    /// that property for all reservations added after initialization.
    pub fn normalize(&mut self) {
        // Remove any accidental zero-length entries.
        let mut compacted = 0usize;

        for index in 0..self.len {
            let region = self.regions[index].unwrap();

            if region.is_empty() {
                continue;
            }

            self.regions[compacted] = Some(region);
            compacted += 1;
        }

        self.len = compacted;

        for index in self.len..Self::MAX_REGIONS {
            self.regions[index] = None;
        }

        // Insertion sort: the map is deliberately small.
        for index in 1..self.len {
            let value = self.regions[index].unwrap();
            let mut position = index;

            while position > 0 {
                let previous = self.regions[position - 1].unwrap();

                if previous.start < value.start
                    || (
                        previous.start == value.start
                            && previous.end() <= value.end()
                    )
                {
                    break;
                }

                self.regions[position] = self.regions[position - 1];
                position -= 1;
            }

            self.regions[position] = Some(value);
        }

        if self.len < 2 {
            return;
        }

        // Coalesce touching/overlapping regions of the same classification.
        let mut output = 0usize;

        for input in 1..self.len {
            let current = self.regions[output].unwrap();
            let next = self.regions[input].unwrap();

            if current.kind == next.kind && next.start <= current.end() {
                let merged_end = current.end().max(next.end());

                self.regions[output] = Some(MemoryRegion {
                    start: current.start,
                    size: merged_end - current.start,
                    kind: current.kind,
                });
            } else {
                /*
                 * Different classifications must not overlap. An overlap here
                 * means a caller used `add()` where it should have used
                 * `overlay()`.
                 */
                debug_assert!(
                    current.end() <= next.start,
                    "overlapping memory regions with different classifications: {:?} and {:?}",
                    current,
                    next,
                );

                output += 1;
                self.regions[output] = Some(next);
            }
        }

        self.len = output + 1;

        for index in self.len..Self::MAX_REGIONS {
            self.regions[index] = None;
        }
    }
}

impl Default for MemoryMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Global memory manager, filled once during early initialization.
pub struct MemoryManager {
    map: MemoryMap,
}

impl MemoryManager {
    pub const fn new() -> Self {
        Self {
            map: MemoryMap::new(),
        }
    }

    #[inline(always)]
    pub fn map(&self) -> &MemoryMap {
        &self.map
    }

    #[inline(always)]
    pub fn map_mut(&mut self) -> &mut MemoryMap {
        &mut self.map
    }

    /// Import the memory information produced by the one-pass boot FDT parser.
    ///
    /// `layout.usable_regions()` already has every fixed firmware/FDT
    /// reservation subtracted. Fixed reservations are overlaid afterward so
    /// the final map remains descriptive while still being non-overlapping.
    ///
    /// Size-only dynamic reservations such as `linux,cma` are intentionally
    /// not imported here because no physical address has been selected for
    /// them yet. They remain available through
    /// `BootInfo::memory.dynamic_reservations()`.
    pub fn init_from_boot_layout(&mut self, layout: &BootMemoryLayout) {
        self.map.clear();

        for range in layout.usable_regions() {
            self.map.add(MemoryRegion {
                start: range.start,
                size: range.size,
                kind: MemoryRegionKind::Usable,
            });
        }

        /*
         * Overlay in increasing priority. If malformed firmware describes
         * overlapping fixed reservations, NoMap wins over Dma, and Dma wins
         * over a generic reservation.
         */
        self.import_reservations_of_kind(
            layout,
            BootMemoryKind::Reserved,
            MemoryRegionKind::Reserved,
        );
        self.import_reservations_of_kind(
            layout,
            BootMemoryKind::Dma,
            MemoryRegionKind::Dma,
        );
        self.import_reservations_of_kind(
            layout,
            BootMemoryKind::NoMap,
            MemoryRegionKind::NoMap,
        );

        self.map.normalize();
    }

    fn import_reservations_of_kind(
        &mut self,
        layout: &BootMemoryLayout,
        source_kind: BootMemoryKind,
        target_kind: MemoryRegionKind,
    ) {
        for reservation in layout
            .reservations()
            .iter()
            .filter(|reservation| reservation.kind == source_kind)
        {
            self.map.overlay(MemoryRegion {
                start: reservation.range.start,
                size: reservation.range.size,
                kind: target_kind,
            });
        }
    }

    /// Reserve a runtime interval such as the kernel image, stack, DTB,
    /// framebuffer, or temporary heap.
    ///
    /// Unlike the previous implementation, this carves the interval out of
    /// every overlapping usable region instead of appending an overlapping
    /// second entry.
    pub fn reserve_range(
        &mut self,
        start: PhysAddr,
        end: PhysAddr,
        kind: MemoryRegionKind,
    ) {
        if end <= start {
            return;
        }

        self.map.overlay(MemoryRegion {
            start,
            size: end - start,
            kind,
        });
    }

    /// Find the smallest usable region that can satisfy `size`.
    pub fn find_usable_region(&self, size: PhysSize) -> Option<&MemoryRegion> {
        self.map
            .iter()
            .filter(|region| {
                region.kind == MemoryRegionKind::Usable
                    && region.size >= size
            })
            .min_by(|left, right| {
                left.size
                    .cmp(&right.size)
                    .then_with(|| left.start.cmp(&right.start))
            })
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton.
///
/// Access remains unsafe, matching the existing kernel API. This can later be
/// wrapped in the same `UnsafeCell`-based singleton abstraction as the other
/// early-boot drivers.
pub static mut MEMORY_MANAGER: MemoryManager = MemoryManager::new();

