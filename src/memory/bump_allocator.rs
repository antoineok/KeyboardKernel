use crate::memory::vmm::Vmm;
use crate::uart_println;
use core::alloc::{GlobalAlloc, Layout};

/// Kernel bump heap backed by virtual memory pages mapped as needed.
pub struct BumpAllocator {
    pub heap_start: u64,
    pub heap_end: u64,
    pub cursor: u64,
    pub vmm: *mut Vmm, // Must be a mutable reference!
}

impl BumpAllocator {
    pub const fn new(heap_start: u64, heap_size: u64, vmm: *mut Vmm) -> Self {
        BumpAllocator {
            heap_start,
            heap_end: heap_start + heap_size,
            cursor: heap_start,
            vmm,
        }
    }

    /// Allocates a chunk of memory, mapping pages as needed.
    pub unsafe fn alloc(&mut self, size: usize, align: usize) -> Option<u64> {
        let mut addr = (self.cursor + (align as u64 - 1)) & !(align as u64 - 1);
        let end = addr + size as u64;
        if end > self.heap_end {
            uart_println!("BumpAllocator out of memory!");
            return None;
        }
        // Map any unmapped heap pages
        let mut page_addr = addr & !(crate::memory::vmm::PAGE_SIZE as u64 - 1);
        while page_addr < end {
            if (&mut *self.vmm).query(page_addr).is_none() {
                let phys = (&mut *self.vmm).pmm.alloc().unwrap();
                (&mut *self.vmm).map(page_addr, phys, crate::memory::vmm::PTE_NX);
                // No-exec
            }
            page_addr += crate::memory::vmm::PAGE_SIZE as u64;
        }
        self.cursor = end;
        Some(addr)
    }

    /// Bump allocator can't deallocate!
    pub unsafe fn dealloc(&mut self, _ptr: u64, _size: usize, _align: usize) {
        // No-op
    }
}

/// Implements the Rust global allocator API for your kernel.
pub struct KernelGlobalAlloc {
    pub allocator: *mut BumpAllocator,
}

unsafe impl GlobalAlloc for KernelGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let bump = &mut *self.allocator;
        bump.alloc(layout.size(), layout.align())
            .map(|addr| addr as *mut u8)
            .unwrap_or(core::ptr::null_mut())
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // No-op for bump allocator
    }
}
