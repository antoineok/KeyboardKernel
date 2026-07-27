//! Physically contiguous buffers shared with non-coherent hardware.
//!
//! DMA buffers are allocated directly from the physical page allocator rather
//! than the Rust heap. Normal RAM remains write-back cacheable, so callers must
//! explicitly synchronize ownership when a device reads or writes the buffer.

use core::{marker::PhantomData, ptr::NonNull, slice};

use crate::arch::{cache, cpu::CpuId};

use super::{phys::PHYS_ALLOCATOR, PhysAddr};

const PAGE_SIZE: usize = 4096;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DmaBufferError {
    ZeroLength,
    InvalidAlignment,
    SizeOverflow,
    OutOfMemory,
}

/// Page-backed, physically contiguous DMA memory.
///
/// The current MMU uses an identity mapping for normal RAM, so `virt_addr()`
/// and `phys_addr()` refer to the same numeric address. Keep both concepts in
/// the API so the video driver does not depend on identity mapping forever.
///
/// Allocation and destruction are intentionally restricted to CPU0 until the
/// physical page allocator itself becomes SMP-safe.
pub struct DmaBuffer {
    ptr: NonNull<u8>,
    phys_addr: PhysAddr,
    len: usize,
    capacity: usize,
    page_count: usize,

    // DMA memory has mutable ownership transitions and must not silently gain
    // Send/Sync semantics merely because it contains an address.
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl DmaBuffer {
    pub fn allocate(len: usize, alignment: usize) -> Result<Self, DmaBufferError> {
        assert_eq!(
            crate::arch::cpu::current_id(),
            CpuId::BOOT,
            "DMA buffers must currently be allocated on CPU0"
        );

        if len == 0 {
            return Err(DmaBufferError::ZeroLength);
        }
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(DmaBufferError::InvalidAlignment);
        }

        let page_count = len
            .checked_add(PAGE_SIZE - 1)
            .ok_or(DmaBufferError::SizeOverflow)?
            / PAGE_SIZE;
        let capacity = page_count
            .checked_mul(PAGE_SIZE)
            .ok_or(DmaBufferError::SizeOverflow)?;
        let effective_alignment = alignment.max(PAGE_SIZE);

        let phys_addr = unsafe {
            PHYS_ALLOCATOR
                .get_mut()
                .alloc_contiguous_aligned(page_count, effective_alignment)
        }
        .ok_or(DmaBufferError::OutOfMemory)?;

        let ptr = NonNull::new(phys_addr as usize as *mut u8)
            .expect("physical allocator returned the null page for DMA memory");

        // Reading uninitialized memory is undefined even for u8. Start every
        // DMA allocation in a defined state and also avoid exposing stale data
        // from a previous physical-page owner to a device.
        unsafe {
            core::ptr::write_bytes(ptr.as_ptr(), 0, capacity);
        }

        Ok(Self {
            ptr,
            phys_addr,
            len,
            capacity,
            page_count,
            _not_send_or_sync: PhantomData,
        })
    }

    #[inline(always)]
    pub const fn phys_addr(&self) -> PhysAddr {
        self.phys_addr
    }

    #[inline(always)]
    pub fn virt_addr(&self) -> usize {
        self.ptr.as_ptr() as usize
    }

    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Publish all CPU writes in the logical buffer to non-coherent hardware.
    pub fn sync_for_device(&self) {
        self.sync_range_for_device(0, self.len);
    }

    /// Publish CPU writes in a subrange before a device reads that range.
    pub fn sync_range_for_device(&self, offset: usize, len: usize) {
        let ptr = self.checked_range_ptr(offset, len);
        unsafe {
            cache::clean_data_range_to_poc(ptr, len);
        }
    }

    /// Discard stale CPU cache lines before reading data written by hardware.
    pub fn sync_for_cpu(&self) {
        self.sync_range_for_cpu(0, self.len);
    }

    /// Discard stale CPU cache lines for a device-written subrange.
    pub fn sync_range_for_cpu(&self, offset: usize, len: usize) {
        let ptr = self.checked_range_ptr(offset, len);
        unsafe {
            cache::invalidate_data_range_to_poc(ptr, len);
        }
    }

    fn checked_range_ptr(&self, offset: usize, len: usize) -> *const u8 {
        let end = offset
            .checked_add(len)
            .expect("DMA synchronization range overflow");
        assert!(end <= self.len, "DMA synchronization range is out of bounds");

        unsafe { self.ptr.as_ptr().add(offset) }
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        assert_eq!(
            crate::arch::cpu::current_id(),
            CpuId::BOOT,
            "DMA buffers must currently be freed on CPU0"
        );

        // The contents are being discarded. Flush and invalidate every page so
        // a later owner cannot inherit dirty/stale cache lines associated with
        // a device transaction that has already finished.
        unsafe {
            cache::clean_invalidate_data_range_to_poc(self.ptr.as_ptr(), self.capacity);
            PHYS_ALLOCATOR
                .get_mut()
                .free_contiguous(self.phys_addr, self.page_count);
        }
    }
}
