// src/memory/heap.rs

//! Kernel heap on top of the physical page allocator.
//!
//! - Uses a contiguous region obtained from PHYS_ALLOCATOR.alloc_contiguous.
//! - Implements a free-list allocator with coalescing.
//! - Backing region is identity-mapped (phys == virt).
//!
//! This is *not* a fancy allocator, but good enough to back `GlobalAlloc`
//! and allow `Vec`, `Box`, etc. to work without leaking everything.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;

use crate::memory::phys::PHYS_ALLOCATOR;
use crate::memory::PhysSize;

const PAGE_SIZE: PhysSize = 4096;
const HEAP_PAGES: usize = 1024; // 4 MiB heap by default: 1024 * 4 KiB

#[repr(C)]
struct FreeBlock {
    size: usize,          // total size of this free block, in bytes (including header)
    next: *mut FreeBlock, // next free block in ascending address order
}

#[repr(C)]
struct AllocHeader {
    size: usize, // total size of this allocation (header + payload + padding)
}

pub struct KernelHeap {
    start: usize,
    end: usize,
    free_list: *mut FreeBlock,
    initialized: bool,
}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            start: 0,
            end: 0,
            free_list: core::ptr::null_mut(),
            initialized: false,
        }
    }

    /// Initialize the heap by reserving a contiguous chunk of physical pages
    /// from the physical allocator.
    ///
    /// Must be called once, after PHYS_ALLOCATOR is initialized and MMU is on
    /// (identity mapping).
    pub unsafe fn init(&mut self) {
        if self.initialized {
            return;
        }

        let pages = HEAP_PAGES;
        let pa = PHYS_ALLOCATOR.get_mut()
            .alloc_contiguous(pages)
            .expect("KernelHeap: failed to allocate heap region from PHYS_ALLOCATOR");

        let size_bytes = (pages as u64) * PAGE_SIZE;

        // Identity map: phys == virt for now.
        self.start = pa as usize;
        self.end = (pa + size_bytes) as usize;

        // One big free block covering the whole heap.
        let first = self.start as *mut FreeBlock;
        (*first).size = size_bytes as usize;
        (*first).next = core::ptr::null_mut();

        self.free_list = first;
        self.initialized = true;
    }

    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return null_mut();
        }

        let header_size = core::mem::size_of::<AllocHeader>();
        let header_align = core::mem::align_of::<AllocHeader>();
        let align = layout.align().max(header_align);
        let size = layout.size().max(core::mem::size_of::<usize>());

        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut curr = self.free_list;

        while !curr.is_null() {
            let block_start = curr as usize;
            let block_size = (*curr).size;
            let block_end = block_start + block_size;

            // We want:
            // [block_start .... alloc_start | header | user_data ... alloc_end) ... block_end
            //
            // Choose user_ptr so that:
            //   - user_ptr is aligned to `align`
            //   - header is immediately before user_ptr
            //
            // So:
            //   user_ptr = align_up(block_start + header_size, align)
            //   header_ptr = user_ptr - header_size
            let user_ptr = align_up(block_start + header_size, align);
            let header_ptr = user_ptr - header_size;
            let alloc_start = header_ptr;

            // Check for overflow of user area.
            let raw_alloc_end = match user_ptr.checked_add(size) {
                Some(v) => v,
                None => {
                    prev = curr;
                    curr = (*curr).next;
                    continue;
                }
            };

            let alloc_end = align_up(
                raw_alloc_end,
                core::mem::align_of::<FreeBlock>(),
            );

            if alloc_end > block_end {
                // Doesn't fit in this block; try next.
                prev = curr;
                curr = (*curr).next;
                continue;
            }

            // We found a suitable block.
            let alloc_size = alloc_end - alloc_start;

            // Remove this block from free list.
            let next = (*curr).next;
            if prev.is_null() {
                self.free_list = next;
            } else {
                (*prev).next = next;
            }

            // Split front fragment (if any).
            let front_size = alloc_start.saturating_sub(block_start);
            if front_size >= core::mem::size_of::<FreeBlock>() {
                self.insert_free_block(block_start, front_size);
            }

            // Split back fragment (if any).
            let back_size = block_end.saturating_sub(alloc_end);
            if back_size >= core::mem::size_of::<FreeBlock>() {
                self.insert_free_block(alloc_end, back_size);
            }

            // Write allocation header
            let header = header_ptr as *mut AllocHeader;
            (*header).size = alloc_size;

            return user_ptr as *mut u8;
        }

        // No suitable block found.
        null_mut()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
        if !self.initialized || ptr.is_null() {
            return;
        }

        let header_size = core::mem::size_of::<AllocHeader>();
        let header_ptr = (ptr as usize).saturating_sub(header_size) as *mut AllocHeader;
        let alloc_size = (*header_ptr).size;
        let alloc_start = header_ptr as usize;

        if alloc_size == 0 {
            return;
        }

        self.insert_free_block(alloc_start, alloc_size);
    }

    /// Insert a free block [start, start + size) into the free list,
    /// sorted by address, and coalesce with neighbors if adjacent.
    unsafe fn insert_free_block(&mut self, start: usize, size: usize) {
        if size < core::mem::size_of::<FreeBlock>() {
            // Too small to be useful.
            return;
        }

        let mut block = start as *mut FreeBlock;
        (*block).size = size;
        (*block).next = core::ptr::null_mut();

        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut curr = self.free_list;

        // Find insertion point (keep list sorted by address).
        while !curr.is_null() && (curr as usize) < start {
            prev = curr;
            curr = (*curr).next;
        }

        // Insert between prev and curr.
        (*block).next = curr;
        if prev.is_null() {
            self.free_list = block;
        } else {
            (*prev).next = block;
        }

        // Coalesce with next (curr) if adjacent.
        if !curr.is_null() {
            let block_end = start + size;
            let curr_start = curr as usize;
            if block_end == curr_start {
                // Merge block and curr.
                (*block).size += (*curr).size;
                (*block).next = (*curr).next;
                curr = (*block).next;
            }
        }

        // Coalesce with prev if adjacent.
        if !prev.is_null() {
            let prev_start = prev as usize;
            let prev_end = prev_start + (*prev).size;
            let block_start = block as usize;

            if prev_end == block_start {
                // Merge prev and block.
                (*prev).size += (*block).size;
                (*prev).next = (*block).next;
                block = prev;
            }
        }
    }
}

/// Simple align_up helper.
fn align_up(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    let mask = align - 1;
    (addr + mask) & !mask
}
pub struct KernelHeapStatic(UnsafeCell<KernelHeap>);

unsafe impl Sync for KernelHeapStatic {}

pub static KERNEL_HEAP: KernelHeapStatic = KernelHeapStatic(UnsafeCell::new(KernelHeap::new()));

impl KernelHeapStatic {
    pub fn get_mut(&self) -> &mut KernelHeap {
        unsafe { &mut *self.0.get() }
    }

    pub fn get(&self) -> &KernelHeap {
        unsafe { &*self.0.get() }
    }
}

pub struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        KERNEL_HEAP.get_mut().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        KERNEL_HEAP.get_mut().dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: KernelAllocator = KernelAllocator;

/// Public init entry for the heap.
pub unsafe fn init_heap() {
    KERNEL_HEAP.get_mut().init();
}
