use crate::{
    arch::{gic::{GIC_COMPATIBLE, GIC_PLATFORM}, mmu},
    drivers::{emmc::EMMC, gpio::GPIO_REGS, mailbox, uart::UART},
    early_timeit,
    fdt::BootInfo,
    graphics::framebuffer::FRAMEBUFFER,
    memory::{boot::BOOT_ALLOCATOR, heap, phys::PHYS_ALLOCATOR, MemoryRegionKind, MEMORY_MANAGER},
    storage::{
        devices::{BOOT_SLICE, EMMC_DEV, ROM_SLICE},
        fs::vfs::{FsId, VFS},
        partition::mbr,
    },
    video::hevc::{HEVC_COMPATIBLE, HEVC_PLATFORM},
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

const REQUIRED_ALIASES: &[&str] = &["gpio", "aux", "mailbox", "mmc0", "watchdog", "usb"];
const REQUIRED_DEVICES: &[&str] = &[HEVC_COMPATIBLE, GIC_COMPATIBLE];

pub unsafe fn early_el1_init(device_tree_ptr: usize) {
    let boot_info = early_timeit!("Parse boot FDT", {
        unsafe {
            BootInfo::from_ptr(
                device_tree_ptr as *const u8,
                REQUIRED_ALIASES,
                REQUIRED_DEVICES,
            )
                .expect("Invalid boot FDT")
        }
    });

    let gpio = GPIO_REGS.get_mut();
    early_timeit!("GPIO init", { gpio.init(boot_info.get("gpio")) });

    let uart = UART.get_mut();
    early_timeit!("UART init", { uart.init(boot_info.get("aux")) });

    #[cfg(feature = "jtag")]
    {
        crate::jtag::init_jtag();
        crate::info!("JTAG initialized");
    }

    let mailbox = mailbox::MAILBOX.get();
    early_timeit!("Mailbox init", { mailbox.init(boot_info.get("mailbox")) });

    let framebuffer = FRAMEBUFFER.get();
    early_timeit!("Framebuffer init", {
        framebuffer.init();
    });

    let emmc = &mut EMMC.get_mut();
    early_timeit!("eMMC init from FDT", {
        emmc.init(boot_info.get("mmc0"));
    });
    if !emmc.is_initialized() {
        panic!("EMMC FAILED TO INITIALIZE!");
    }

    let watchdog = crate::drivers::watchdog::WATCHDOG.get_mut();
    early_timeit!("Watchdog init from FDT", {
        watchdog.init(boot_info.get("watchdog"));
    });
    if !watchdog.is_initialized() {
        panic!("Watchdog FAILED TO INITIALIZE!");
    }

    early_timeit!("HEVC decoder discovery", {
        HEVC_PLATFORM.init(boot_info.device(HEVC_COMPATIBLE));
    });

    early_timeit!("GICv2 discovery", {
        GIC_PLATFORM.init(boot_info.device(GIC_COMPATIBLE));
    });

    //let bt = crate::drivers::bluetooth::BLUETOOTH.get();
    //bt.init(mmio_table.entries.get("bluetooth"));

    early_timeit!("Memory manager init from FDT", {
        MEMORY_MANAGER
            .get_mut()
            .init_from_boot_layout(&boot_info.memory);
    });

    early_timeit!("Kernel memory reservation", {
        let mm = MEMORY_MANAGER.get_mut();

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

        // The framebuffer is allocated by VideoCore, but exposed to the ARM as
        // ordinary physical memory. Keep those pages out of PHYS_ALLOCATOR and
        // classify them as shared DMA memory for the MMU.
        if !framebuffer.buffer().is_null() && framebuffer.buffer_size() != 0 {
            let framebuffer_start = framebuffer.buffer() as u64;
            let framebuffer_end = framebuffer_start
                .checked_add(framebuffer.buffer_size() as u64)
                .expect("Framebuffer reservation overflow");

            mm.reserve_range(framebuffer_start, framebuffer_end, MemoryRegionKind::Dma);
        }

        // Re-normalize now that we added regions
        mm.map_mut().normalize();
    });

    early_timeit!("Boot allocator init", {
        init_boot_allocator();
    });

    crate::info!("Building EL1 identity map");
    let mmu = early_timeit!("MMU page-table build", { mmu::map_kernel_and_mmio() });

    crate::info!(
        "Enabling EL1 MMU with TTBR0={:#018x} and coherent data cache",
        mmu.l1_phys()
    );
    early_timeit!("MMU enable", {
        mmu.enable();
    });
    assert!(mmu::is_enabled(), "EL1 MMU failed to enable");
    assert!(
        mmu::is_data_cache_enabled(),
        "EL1 data cache failed to enable"
    );
    crate::info!("EL1 MMU and data cache enabled");

    early_timeit!("GICv2 register probe", {
        GIC_PLATFORM.probe();
    });

    early_timeit!("HEVC clock enable", {
        HEVC_PLATFORM.enable_clock(mailbox);
    });

    early_timeit!("HEVC register probe", {
        HEVC_PLATFORM.probe_hardware();
    });

    early_timeit!("HEVC interrupt-controller init", {
        HEVC_PLATFORM.initialize_interrupt_controller();
    });

    init_phys_allocator();

    early_timeit!("HEVC decode workspace", {
        crate::video::rpivid::initialize_decode_workspace(1366, 768);
    });

    early_timeit!("USB power domain", {
        if !mailbox.set_power_state(mailbox::POWER_DEVICE_USB_HCD, true, true) {
            panic!("Failed to power the USB controller");
        }
    });

    let usb = crate::drivers::usb::USB.get_mut();

    early_timeit!("DWC2 probe", {
        usb.init(boot_info.get("usb"))
            .expect("DWC2 failed to initialize");
    });

    early_timeit!("DWC2 device mode", {
        usb.prepare_device_mode()
            .expect("DWC2 device-mode preparation failed");
    });

    early_timeit!("VFS init", {
        init_vfs();
    });
}

unsafe fn init_boot_allocator() {
    let base = &__heap_start as *const u8 as u64;

    // Find a usable region that contains `base`
    let limit = &__heap_end as *const u8 as u64;
    let allocator = BOOT_ALLOCATOR.get_mut();
    allocator.init(base, limit);
}

unsafe fn init_phys_allocator() {
    let mm = MEMORY_MANAGER.get().map();
    early_timeit!("Physical allocator init", {
        PHYS_ALLOCATOR.with_mut(|allocator| allocator.init_from_memory_map(mm));
    });

    // 6) Kernel heap
    early_timeit!("Initialize the heap", {
        heap::init_heap();
    });
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

    VFS.with_mut(|vfs| {
        vfs.mount_fat32(FsId::Boot, boot).unwrap();
        vfs.mount_fat32(FsId::Rom, rom).unwrap();
    });
}
