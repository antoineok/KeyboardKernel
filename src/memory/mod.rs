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
