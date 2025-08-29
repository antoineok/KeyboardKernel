use crate::memory::pmm::Pmm;

pub const PAGE_SIZE: usize = 4096;
pub const ENTRIES_PER_TABLE: usize = 512;

pub const PTE_VALID: u64 = 1 << 0;
pub const PTE_TABLE: u64 = 1 << 1;
pub const PTE_PAGE: u64 = 0 << 1;
pub const PTE_AF: u64 = 1 << 10;
pub const PTE_SH_INNER: u64 = 3 << 8;
pub const PTE_NX: u64 = 1 << 54;

/// Page table structure
#[repr(align(4096))]
pub struct PageTable {
    pub entries: [u64; ENTRIES_PER_TABLE],
}

impl PageTable {
    pub fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = 0;
        }
    }
}

pub struct Vmm {
    pub root: *mut PageTable, // TTBR1
    pub pmm: Pmm,
}

impl Vmm {
    pub fn new(mut pmm: Pmm) -> Self {
        let root_pa = pmm.alloc().unwrap();
        let root = root_pa as *mut PageTable;
        unsafe {
            (*root).zero();
        }
        Vmm { root, pmm }
    }

    /// Page table walker: returns (&mut u64, level) for VA, allocates tables if needed
    pub fn walk(&mut self, virt: u64, alloc: bool) -> (&mut u64, usize) {
        let mut table = self.root;
        for lvl in 0..4 {
            let idx = ((virt >> (39 - lvl * 9)) & 0x1FF) as usize;
            let entry = unsafe { &mut (*table).entries[idx] };
            if lvl == 3 {
                return (entry, lvl);
            }
            if (*entry & PTE_VALID) == 0 {
                if !alloc {
                    return (entry, lvl);
                }
                let new_table_pa = self.pmm.alloc().unwrap();
                let new_table = new_table_pa as *mut PageTable;
                unsafe {
                    (*new_table).zero();
                }
                *entry = new_table_pa | PTE_VALID | PTE_TABLE;
            }
            let next_table_pa = *entry & 0x0000_FFFF_FFFF_F000;
            table = next_table_pa as *mut PageTable;
        }
        unreachable!()
    }

    /// Map a virtual address to a physical address
    pub fn map(&mut self, virt: u64, phys: u64, flags: u64) {
        let (entry, level) = self.walk(virt, true);
        *entry = phys | flags | PTE_VALID | PTE_PAGE | PTE_AF | PTE_SH_INNER;
    }

    pub fn unmap(&mut self, virt: u64) {
        let (entry, _) = self.walk(virt, false);
        *entry = 0;
    }

    pub fn query(&mut self, virt: u64) -> Option<u64> {
        let (entry, _) = self.walk(virt, false);
        if (*entry & PTE_VALID) == 0 {
            None
        } else {
            Some(*entry)
        }
    }
}
