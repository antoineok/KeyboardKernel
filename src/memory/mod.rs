// src/memory/mod.rs
#![allow(dead_code)]

pub mod boot;
pub mod heap;
pub mod phys;

use fdt::Fdt;

/// Physical address / size types
pub type PhysAddr = u64;
pub type PhysSize = u64;

/// High-level classification of regions we care about.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MemoryRegionKind {
    /// General-purpose RAM usable for allocations.
    Usable,
    /// Reserved for firmware or special purposes; do not use.
    Reserved,
    /// Reserved for DMA / CMA pools.
    Dma,
    /// Must not be mapped in the MMU (`no-map`).
    NoMap,
}

/// A single physical memory region in our internal representation.
#[derive(Copy, Clone, Debug)]
pub struct MemoryRegion {
    pub start: PhysAddr,
    pub size: PhysSize,
    pub kind: MemoryRegionKind,
}

impl MemoryRegion {
    #[inline]
    pub const fn end(&self) -> PhysAddr {
        self.start + self.size
    }

    #[inline]
    pub const fn contains(&self, addr: PhysAddr) -> bool {
        addr >= self.start && addr < self.end()
    }
}

/// Fixed-size container for memory regions.
/// This avoids heap allocation during early boot.
pub struct MemoryMap {
    regions: [Option<MemoryRegion>; MemoryMap::MAX_REGIONS],
    len: usize,
}

impl MemoryMap {
    pub const MAX_REGIONS: usize = 32;

    pub const fn new() -> Self {
        Self {
            regions: [None; Self::MAX_REGIONS],
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn add(&mut self, region: MemoryRegion) {
        // You can turn this into a debug assert if you prefer.
        assert!(self.len < Self::MAX_REGIONS);
        self.regions[self.len] = Some(region);
        self.len += 1;
    }

    pub fn iter(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions[..self.len].iter().map(|r| r.as_ref().unwrap())
    }

    /// Optional: sort regions by start address, and coalesce adjacent regions
    /// of the same kind. You can call this once after init if you want.
    pub fn normalize(&mut self) {
        // Simple insertion sort, small N (<= 32) so fine.
        for i in 1..self.len {
            let mut j = i;
            while j > 0 {
                let a = self.regions[j - 1].unwrap();
                let b = self.regions[j].unwrap();

                if a.start <= b.start {
                    break;
                }

                self.regions.swap(j - 1, j);
                j -= 1;
            }
        }

        // Coalesce adjacent regions of the same kind when they touch.
        let mut out = 0;
        let mut i = 0;

        while i < self.len {
            let mut cur = self.regions[i].unwrap();
            i += 1;

            while i < self.len {
                let next = self.regions[i].unwrap();
                if cur.kind == next.kind && cur.end() == next.start {
                    cur.size += next.size;
                    i += 1;
                } else {
                    break;
                }
            }

            self.regions[out] = Some(cur);
            out += 1;
        }

        self.len = out;
        for k in self.len..Self::MAX_REGIONS {
            self.regions[k] = None;
        }
    }
}

/// Global memory manager, filled once during early init.
pub struct MemoryManager {
    map: MemoryMap,
}

impl MemoryManager {
    pub const fn new() -> Self {
        Self {
            map: MemoryMap::new(),
        }
    }

    #[inline]
    pub fn map(&self) -> &MemoryMap {
        &self.map
    }

    #[inline]
    pub fn map_mut(&mut self) -> &mut MemoryMap {
        &mut self.map
    }

    pub fn reserve_range(&mut self, start: PhysAddr, end: PhysAddr, kind: MemoryRegionKind) {
        if end <= start {
            return;
        }
        let size = end - start;
        self.map.add(MemoryRegion { start, size, kind });
    }

    /// Main entry: call from your kernel init with a `'static` Fdt.
    ///
    /// Example:
    /// ```ignore
    /// let fdt = unsafe { Fdt::from_ptr(device_tree_ptr as *const u8).unwrap() };
    /// let fdt: Fdt<'static> = unsafe { core::mem::transmute(fdt) };
    /// unsafe { MEMORY_MANAGER.init_from_fdt(&fdt); }
    /// ```
    pub fn init_from_fdt(&mut self, fdt: &Fdt<'static>) {
        self.parse_ram(fdt);
        self.parse_memory_reservations(fdt);
        self.parse_reserved_memory_node(fdt);

        // Optional: sort/merge
        self.map.normalize();
    }

    /// Parse `/memory` via the crate's standard helper.
    fn parse_ram(&mut self, fdt: &Fdt<'static>) {
        let mem = fdt.memory();

        for region in mem.regions() {
            // fdt::standard_nodes::MemoryRegion:
            //   starting_address: *const u8
            //   size: Option<usize>
            let start = region.starting_address as usize as u64;
            let size = match region.size {
                Some(sz) if sz != 0 => sz as u64,
                _ => continue,
            };

            self.map.add(MemoryRegion {
                start,
                size,
                kind: MemoryRegionKind::Usable,
            });
        }
    }

    /// Parse top-level memory reservation blocks (memreserve entries).
    fn parse_memory_reservations(&mut self, fdt: &Fdt<'static>) {
        for res in fdt.memory_reservations() {
            let addr = res.address() as usize as u64;
            let size = res.size() as u64;

            if size == 0 {
                continue;
            }

            self.map.add(MemoryRegion {
                start: addr,
                size,
                kind: MemoryRegionKind::Reserved,
            });
        }
    }

    /// Parse the `/reserved-memory` node and its children.
    ///
    /// This handles:
    /// - `reg` → Reserved / Dma / NoMap
    /// - `compatible = "shared-dma-pool"` → Dma
    /// - `no-map` → NoMap
    /// - `status = "disabled"` → ignored
    ///
    /// Nodes like your linux,cma without `reg` are currently ignored,
    /// since they describe *allocatable* pools, not fixed regions.
    fn parse_reserved_memory_node(&mut self, fdt: &Fdt<'static>) {
        let reserved_opt = fdt.find_node("/reserved-memory");
        let reserved = match reserved_opt {
            Some(n) => n,
            None => return,
        };

        for child in reserved.children() {
            // status = "disabled" → ignore
            let status = child
                .property("status")
                .and_then(|p| p.as_str())
                .unwrap_or("okay");

            if status == "disabled" {
                continue;
            }

            let compatible = child.property("compatible").and_then(|p| p.as_str());

            let has_no_map = child.property("no-map").is_some();

            // If there's a reg / raw_reg, interpret it as static reserved region(s).
            if let Some(raw_iter) = child.raw_reg() {
                for raw in raw_iter {
                    let addr = self.be_bytes_to_u64(raw.address);
                    let size = self.be_bytes_to_u64(raw.size);

                    if size == 0 {
                        continue;
                    }

                    let kind = if has_no_map {
                        MemoryRegionKind::NoMap
                    } else if compatible
                        .map(|s| s.contains("shared-dma-pool"))
                        .unwrap_or(false)
                    {
                        MemoryRegionKind::Dma
                    } else {
                        MemoryRegionKind::Reserved
                    };

                    self.map.add(MemoryRegion {
                        start: addr,
                        size,
                        kind,
                    });
                }
            } else if compatible
                .map(|s| s.contains("shared-dma-pool"))
                .unwrap_or(false)
            {
                // CMA-style node with no fixed `reg`:
                // something like:
                //   linux,cma {
                //       size = <0x4000000>;
                //       alloc-ranges = <0x00 0x00 0x30000000>;
                //   };
                //
                // For now we *do not* carve it out of the usable regions,
                // because the pool is allocated dynamically by the OS.
                // You can later add explicit CMA handling here if you want.
                let _ = child.property("size").and_then(|p| p.as_usize());
                let _ = child.property("alloc-ranges");
            }
        }
    }

    /// Convert a big-endian cell sequence (as bytes) into a u64.
    ///
    /// The FDT crate already guarantees that `address` and `size` are
    /// multiples of 4 bytes, up to 64 bits. We just need to parse them.
    fn be_bytes_to_u64(&self, bytes: &[u8]) -> u64 {
        let mut out = 0u64;
        for chunk in bytes {
            out = (out << 8) | (*chunk as u64);
        }
        out
    }

    /// Helper to find the first usable region with at least `size` bytes.
    ///
    /// You can build your early allocator on top of this later.
    pub fn find_usable_region(&self, size: PhysSize) -> Option<&MemoryRegion> {
        self.map
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable && r.size >= size)
            .min_by(|a, b| {
                if a.size == b.size {
                    a.start.cmp(&b.start)
                } else {
                    a.size.cmp(&b.size)
                }
            })
    }
}

/// Global singleton. You access it with `unsafe { &mut MEMORY_MANAGER }`
/// from your init code (or wrap that in your own safe API if you prefer).
pub static mut MEMORY_MANAGER: MemoryManager = MemoryManager::new();
