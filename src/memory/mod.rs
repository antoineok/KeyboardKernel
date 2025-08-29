pub mod bump_allocator;
pub mod dt;
pub mod pmm;
pub mod vmm;

use crate::{
    memory::{bump_allocator::KernelGlobalAlloc, dt::Fdt, pmm::Pmm, vmm::Vmm},
    uart_println,
};
use alloc::boxed::Box;
use bump_allocator::BumpAllocator;

static mut BUMP_ALLOC: Option<BumpAllocator> = None;

#[global_allocator]
static mut GLOBAL_ALLOC: KernelGlobalAlloc = KernelGlobalAlloc {
    allocator: 0 as *mut BumpAllocator,
};

pub fn memory_init(dtb_addr: usize) {
    uart_println!("DTB address: {:#x}", dtb_addr);
    let fdt = unsafe { Fdt::new(dtb_addr) };
    if fdt.is_none() {
        panic!("Failed to parse FDT header.");
    }
    let mut fdt = fdt.unwrap();
    unsafe {
        fdt.parse_all(&[
            "arm,pl011",
            "brcm,bcm2835-spi",
            "brcm,bcm2835-aux-spi",
            "brcm,bcm2708-usb",
            "generic-xhci",
            // Add more if desired!
        ]);
        fdt.print_all();
    }
    uart_println!("Usable RAM regions found: {}", fdt.regions_found);
    uart_println!("Reserved RAM regions found: {}", fdt.reserved_found);

    uart_println!("Starting physical memory manager initialization...");
    let pmm = Pmm::new(
        &fdt.regions[..fdt.regions_found],
        &fdt.reserved[..fdt.reserved_found],
    );
    uart_println!("Physical memory manager initialized.");

    uart_println!("Starting virtual memory manager initialization...");
    let mut vmm = Vmm::new(pmm);
    uart_println!("Virtual memory manager initialized.");
    uart_println!("Starting MMU initialization...");
    unsafe {
        enable_mmu(vmm.root as u64); // Pass physical address of root PT
    }
    uart_println!("MMU initialized.");

    // 4. Initialize bump allocator for heap (1MB at 0xffff_8000_0000_0000)
    uart_println!("Starting bump allocator initialization...");
    let heap_start: u64 = 0xffff_8000_0000_0000;
    let heap_size: u64 = 1024 * 1024;
    unsafe {
        BUMP_ALLOC = Some(BumpAllocator::new(
            heap_start,
            heap_size,
            &mut vmm as *mut Vmm,
        ));
        GLOBAL_ALLOC.allocator = BUMP_ALLOC.as_mut().unwrap() as *mut BumpAllocator;
    }
    uart_println!("Bump allocator initialized.");

    uart_println!("Testing heap allocation...");
    let boxed_val = Box::new(42u64);
    uart_println!("Allocated: {}", *boxed_val);
    assert_eq!(*boxed_val, 42);
    drop(boxed_val);
    uart_println!("Heap allocation test passed.");
}

pub unsafe fn enable_mmu(root_table_pa: u64) {
    use core::arch::asm;

    // Set MAIR_EL1: Attr0=normal memory, Attr1=device memory
    // Attr0 = 0xff (write-back, write-allocate), Attr1 = 0x00 (device nGnRE)
    let mair_value: u64 = (0xff << 0) | (0x00 << 8);
    asm!("msr mair_el1, {0}", in(reg) mair_value);

    // Set TCR_EL1: 4KB granule, 48-bit VA/PA, inner shareable, WBWA
    // This configures TTBR1 (kernel space) only.
    let tcr_value: u64 = (16 << 0) |   // T1SZ: 48 - VA bits
        (1 << 8)  |   // Inner shareable
        (1 << 10) |   // TG1: 4KB granule
        (1 << 12) |   // IPS: 40 bits PA (change if you want 48)
        (1 << 14); // TBI=1 (top byte ignored, optional)
    asm!("msr tcr_el1, {0}", in(reg) tcr_value);

    // Set TTBR1_EL1 to root page table physical address
    asm!("msr ttbr1_el1, {0}", in(reg) root_table_pa);

    // Enable MMU: set SCTLR_EL1.M
    let mut sctlr: u64;
    asm!("mrs {0}, sctlr_el1", out(reg) sctlr);
    sctlr |= 1; // Set M bit (MMU enable)
    asm!("msr sctlr_el1, {0}", in(reg) sctlr);

    // ISB: flush pipeline
    asm!("isb");
}
