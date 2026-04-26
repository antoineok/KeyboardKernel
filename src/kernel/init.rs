use fdt::Fdt;

use crate::{
    drivers::{emmc::EMMC, mailbox, uart::UART}, fdt::aliases::AliasesTable, graphics::framebuffer::FRAMEBUFFER, info, memory::{MEMORY_MANAGER, MemoryRegionKind, boot::BOOT_ALLOCATOR, heap, phys::PHYS_ALLOCATOR}, storage::{devices::{BOOT_SLICE, EMMC_DEV, ROM_SLICE}, fs::vfs::{FsId, VFS}, partition::mbr}, timeit
};

extern "C" {
    // End of kernel image (.text + .rodata + .data + .bss)
    static __kernel_start: u8;
    static __kernel_end: u8;

    // Stacks
    static __el2_stack_start: u8;
    static __el2_stack_top: u8;
    static __el1_stack_start: u8;
    static __el1_stack_top: u8;

    // Temporary heap
    static __heap_start: u8;
    static __heap_end: u8;
}

pub unsafe fn early_el1_init(device_tree_ptr: usize) {
    let fdt = unsafe { Fdt::from_ptr(device_tree_ptr as *const u8).unwrap() };

    let mmio_table = AliasesTable::new(&fdt);

    let gpio = crate::drivers::gpio::regs();
    gpio.init(mmio_table.get("gpio"));

    let uart = UART.get_mut();
    uart.init(mmio_table.get("aux"));
    info!("Logger initialized");

    #[cfg(feature = "jtag")]
    {
        crate::jtag::init_jtag();
        crate::info!("JTAG initialized");
    }

    let mailbox = mailbox::MAILBOX.get();
    timeit!("Mailbox init", { mailbox.init(mmio_table.get("mailbox")) });

    let framebuffer = FRAMEBUFFER.get();
    timeit!("Framebuffer init", {
        framebuffer.init();
    });

    let emmc = &mut EMMC.get_mut();
    timeit!("eMMC init from FDT", {
        emmc.init(mmio_table.get("mmc0"));
    });
    if !emmc.is_initialized() {
        panic!("EMMC FAILED TO INITIALIZE!");
    }

    //let bt = crate::drivers::bluetooth::BLUETOOTH.get();
    //bt.init(mmio_table.entries.get("bluetooth"));

    timeit!("Memory manager init from FDT", {
        MEMORY_MANAGER.init_from_fdt(&fdt);
    });

    timeit!("Kernel memory reservation", {
        let mm = &mut MEMORY_MANAGER;

        // Kernel image: from load address up to _end
        // If you have _kernel_start, use that. Otherwise, assume 0x80000 for now:
        let kernel_start: u64 = &__kernel_start as *const u8 as u64;
        let kernel_end: u64 = &__kernel_end as *const u8 as u64;

        mm.reserve_range(kernel_start, kernel_end, MemoryRegionKind::Reserved);

        // EL2 stack
        let el2_start = &__el2_stack_start as *const u8 as u64;
        let el2_end = &__el2_stack_top as *const u8 as u64;
        mm.reserve_range(el2_start, el2_end, MemoryRegionKind::Reserved);

        // EL1 stack
        let el1_start = &__el1_stack_start as *const u8 as u64;
        let el1_end = &__el1_stack_top as *const u8 as u64;
        mm.reserve_range(el1_start, el1_end, MemoryRegionKind::Reserved);

        // Temporary heap
        let heap_start = &__heap_start as *const u8 as u64;
        let heap_end = &__heap_end as *const u8 as u64;
        mm.reserve_range(heap_start, heap_end, MemoryRegionKind::Reserved);

        // Re-normalize now that we added regions
        mm.map_mut().normalize();
    });

    timeit!("Boot allocator init", {
        init_boot_allocator();
    });

    init_phys_allocator();

        timeit!("VFS init", {
        init_vfs();
    });

}

unsafe fn init_boot_allocator() {
    let base = &__heap_start as *const u8 as u64;

    // Find a usable region that contains `base`
    let limit = &__heap_end as *const u8 as u64;
    BOOT_ALLOCATOR.init(base, limit);
}

unsafe fn init_phys_allocator() {
    let mm = MEMORY_MANAGER.map();
    timeit!("Physical allocator init", {
        PHYS_ALLOCATOR.init_from_memory_map(mm);
    });
    if let Some((start, end, bytes)) = PHYS_ALLOCATOR.info() {
        info!(
            "PhysAllocator arena: [0x{:016x} - 0x{:016x}) size = {:#x}",
            start, end, bytes
        );
    }

    // 6) Kernel heap
    timeit!("Initialize the heap", {
        heap::init_heap();
    });
    info!("Kernel heap initialized.");
}

pub fn init_storage_slices() {
    let dev = EMMC_DEV.get();

    let parts = mbr::read_partitions(dev).unwrap();

    let boot_part = parts.iter().find(|p| p.index == 1).unwrap();
    let rom_part = parts.iter().find(|p| p.index == 2).unwrap();

    // Build slices that borrow the global dev => now slices can be 'static-borrowed
    let boot = boot_part.as_block_device(dev);
    let rom = rom_part.as_block_device(dev);

    *BOOT_SLICE.get_mut() = Some(boot);
    *ROM_SLICE.get_mut() = Some(rom);
}

pub fn init_vfs() {
    init_storage_slices();

    let boot = BOOT_SLICE.get().as_ref().unwrap();
    let rom = ROM_SLICE.get().as_ref().unwrap();

    let vfs = VFS.get_mut();
    vfs.mount_fat32(FsId::Boot, boot).unwrap();
    vfs.mount_fat32(FsId::Rom, rom).unwrap();
}