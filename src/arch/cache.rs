//! AArch64 data-cache maintenance helpers for the BCM2711 Cortex-A72.
//!
//! Normal kernel RAM is mapped write-back cacheable. Buffers shared with
//! non-coherent devices must therefore be cleaned before the device reads them
//! and invalidated before the CPU consumes device-written data.

use core::arch::asm;

const SCTLR_C: u64 = 1 << 2;

#[inline(always)]
pub fn data_cache_enabled() -> bool {
    let sctlr: u64;

    unsafe {
        asm!(
            "mrs {value}, sctlr_el1",
            value = out(reg) sctlr,
            options(nomem, nostack, preserves_flags),
        );
    }

    sctlr & SCTLR_C != 0
}

#[inline(always)]
fn data_cache_line_size() -> usize {
    let ctr: u64;

    unsafe {
        asm!(
            "mrs {value}, ctr_el0",
            value = out(reg) ctr,
            options(nomem, nostack, preserves_flags),
        );
    }

    // CTR_EL0.DminLine is log2(number of 32-bit words in the smallest
    // data/unified cache line). Cortex-A72 reports 4, i.e. 64-byte lines.
    4usize << (((ctr >> 16) & 0xf) as usize)
}

/// Clean every cache line intersecting `[ptr, ptr + len)` to the point of
/// coherency so a non-coherent device observes the CPU's latest writes.
///
/// # Safety
///
/// `ptr..ptr+len` must designate a valid mapped range for the duration of the
/// operation. Callers sharing partial cache lines with unrelated writable data
/// must provide their own synchronization.
pub unsafe fn clean_data_range_to_poc(ptr: *const u8, len: usize) {
    if len == 0 || !data_cache_enabled() {
        return;
    }

    let line_size = data_cache_line_size();
    let start = (ptr as usize) & !(line_size - 1);
    let end = (ptr as usize)
        .checked_add(len)
        .expect("data-cache clean range overflow");

    let mut address = start;
    while address < end {
        asm!(
            "dc cvac, {address}",
            address = in(reg) address,
            options(nostack, preserves_flags),
        );
        address = address
            .checked_add(line_size)
            .expect("data-cache clean address overflow");
    }

    asm!("dsb sy", options(nostack, preserves_flags));
}

/// Invalidate every cache line intersecting `[ptr, ptr + len)` so subsequent
/// CPU reads observe data written by a non-coherent device.
///
/// # Safety
///
/// The range must not contain dirty CPU data that has not already been cleaned
/// to memory. Prefer dedicated cache-line-aligned shared buffers.
pub unsafe fn invalidate_data_range_to_poc(ptr: *const u8, len: usize) {
    if len == 0 || !data_cache_enabled() {
        return;
    }

    // Complete any preceding device/MMIO observations before discarding stale
    // CPU cache lines for the response buffer.
    asm!("dsb sy", options(nostack, preserves_flags));

    let line_size = data_cache_line_size();
    let start = (ptr as usize) & !(line_size - 1);
    let end = (ptr as usize)
        .checked_add(len)
        .expect("data-cache invalidate range overflow");

    let mut address = start;
    while address < end {
        asm!(
            "dc ivac, {address}",
            address = in(reg) address,
            options(nostack, preserves_flags),
        );
        address = address
            .checked_add(line_size)
            .expect("data-cache invalidate address overflow");
    }

    asm!("dsb sy", options(nostack, preserves_flags));
}

/// Clean and invalidate every cache line intersecting `[ptr, ptr + len)`.
///
/// This is primarily useful when retiring a buffer that has crossed a
/// CPU/device ownership boundary and its contents are no longer needed.
///
/// # Safety
///
/// `ptr..ptr+len` must designate a valid mapped range for the duration of the
/// operation.
pub unsafe fn clean_invalidate_data_range_to_poc(ptr: *const u8, len: usize) {
    if len == 0 || !data_cache_enabled() {
        return;
    }

    let line_size = data_cache_line_size();
    let start = (ptr as usize) & !(line_size - 1);
    let end = (ptr as usize)
        .checked_add(len)
        .expect("data-cache clean/invalidate range overflow");

    let mut address = start;
    while address < end {
        asm!(
            "dc civac, {address}",
            address = in(reg) address,
            options(nostack, preserves_flags),
        );
        address = address
            .checked_add(line_size)
            .expect("data-cache clean/invalidate address overflow");
    }

    asm!("dsb sy", options(nostack, preserves_flags));
}
