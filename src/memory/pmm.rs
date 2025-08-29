use crate::memory::dt::{MemRegion, ReservedRegion};

pub const PAGE_SIZE: usize = 4096;
pub const MAX_REGIONS: usize = 8;

pub struct PmmRegion {
    pub base: u64,
    pub size: u64,
    pub total_pages: usize,
    pub bitmap: &'static mut [u8],
}

pub struct Pmm {
    regions: [Option<PmmRegion>; MAX_REGIONS],
    regions_found: usize,
}

impl Pmm {
    pub fn new(ram: &[MemRegion], reserved: &[ReservedRegion]) -> Self {
        let mut pmm = Pmm {
            regions: [None, None, None, None, None, None, None, None],
            regions_found: 0,
        };
        for (i, region) in ram.iter().enumerate() {
            if i >= MAX_REGIONS {
                break;
            }
            let total_pages = (region.size / PAGE_SIZE as u64) as usize;
            static mut BITMAPS: [[u8; 0x40000]; MAX_REGIONS] = [[0; 0x40000]; MAX_REGIONS];
            let bitmap = unsafe { &mut BITMAPS[i][..(total_pages + 7) / 8] };

            // Mark reserved pages
            for res in reserved {
                let start = ((res.base.saturating_sub(region.base)) / PAGE_SIZE as u64) as usize;
                let end =
                    ((res.base + res.size).saturating_sub(region.base) / PAGE_SIZE as u64) as usize;
                for p in start..end {
                    if p < total_pages {
                        bitmap[p / 8] |= 1 << (p % 8);
                    }
                }
            }
            pmm.regions[i] = Some(PmmRegion {
                base: region.base,
                size: region.size,
                total_pages,
                bitmap,
            });
            pmm.regions_found += 1;
        }
        pmm
    }

    pub fn alloc(&mut self) -> Option<u64> {
        for region in self.regions.iter_mut().flatten() {
            for i in 0..region.total_pages {
                let byte = region.bitmap[i / 8];
                let bit = 1 << (i % 8);
                if (byte & bit) == 0 {
                    region.bitmap[i / 8] |= bit;
                    return Some(region.base + (i as u64) * PAGE_SIZE as u64);
                }
            }
        }
        None
    }

    pub fn free(&mut self, addr: u64) {
        for region in self.regions.iter_mut().flatten() {
            if addr >= region.base && addr < region.base + region.size {
                let page = ((addr - region.base) / PAGE_SIZE as u64) as usize;
                if page < region.total_pages {
                    region.bitmap[page / 8] &= !(1 << (page % 8));
                }
            }
        }
    }

    pub fn used(&self) -> usize {
        self.regions
            .iter()
            .flatten()
            .map(|region| {
                (0..region.total_pages)
                    .filter(|i| (region.bitmap[i / 8] & (1 << (i % 8))) != 0)
                    .count()
            })
            .sum()
    }

    pub fn free_pages(&self) -> usize {
        self.regions
            .iter()
            .flatten()
            .map(|region| region.total_pages)
            .sum::<usize>()
            - self.used()
    }
}
