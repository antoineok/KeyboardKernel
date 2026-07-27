pub mod init;

use core::{hint::spin_loop, time::Duration};

use crate::arch::{
    cpu::{self, CpuId},
    exceptions::PrivilegeLevel,
};

use crate::graphics::{graphics_core_ready, GraphicsCoreProxy};
use crate::memory::{
    heap::KERNEL_HEAP, phys::PHYS_ALLOCATOR, MemoryRegionKind, MEMORY_MANAGER,
};
use crate::storage::fs::vfs::FsId;
use crate::time::time_manager;

use crate::{
    early_timeit,
    info,
    services::{KernelManagementBackend, KeyworkServices},
    transport::{usb_core_ready, KeyworkTransport, TransportState, UsbCoreProxy},
    usb::MANAGEMENT_FRAME_CAPACITY,
};

#[cfg(feature = "jtag")]
use crate::jtag::breakpoint;

#[no_mangle]
pub extern "C" fn el1_entry(device_tree_ptr: usize) -> ! {
    // We are now in EL1, with a basic stack (set by EL2 code).
    unsafe {
        early_timeit!("Kernel early EL1 init", {
            init::early_el1_init(device_tree_ptr)
        });

        crate::util::early_timing::flush_early_timings();
    }

    info!("DAIF = {:#x}", read_daif());

    #[cfg(feature = "jtag")]
    breakpoint();

    info!(
        "Current privilege level: {}",
        PrivilegeLevel::current().as_str()
    );
    info!("Current CPU: {}", crate::arch::cpu::current_id());

    bring_up_secondary_cpu(1);
    wait_for_graphics_core();
    bring_up_secondary_cpu(2);
    wait_for_usb_transport_core();

    let mut usb_transport = UsbCoreProxy::new();
    let mut services = KeyworkServices::<MANAGEMENT_FRAME_CAPACITY>::new();
    let mut management_backend = KernelManagementBackend::new();
    let mut graphics = GraphicsCoreProxy::new();

    // CPU0 only publishes the desired visual state. Once queued, CPU1 owns
    // opening/streaming the asset, RPIVID, composition and presentation. There
    // is deliberately no CPU0 acknowledgement poll in the permanent loop.
    graphics
        .start_video_detached(FsId::Rom, "output.kwv")
        .expect("failed to queue KWVIDEO background for CPU1");
    info!("CPU0 queued KWVIDEO background for CPU1");

    log_memory_summary();

    loop {
        let transition = services
            .update_transport_selection(usb_transport.state(), TransportState::Disconnected);
        if transition.changed() {
            info!(
                "Active host transport changed: {:?} -> {:?}",
                transition.previous, transition.current
            );
        }
        management_backend.set_active_transport(services.active_transport());
        services
            .poll_selected_transport(&mut usb_transport, &mut management_backend)
            .expect("USB service polling failed");
        management_backend.poll_pending_action();
    }
}

fn wait_for_graphics_core() {
    const GRAPHICS_CORE_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let start = time_manager().uptime();
    while !graphics_core_ready() {
        if time_manager().uptime().saturating_sub(start) >= GRAPHICS_CORE_BOOT_TIMEOUT {
            panic!("timed out waiting for CPU1 graphics worker");
        }
        spin_loop();
    }

    info!("CPU1 graphics worker handshake complete");
}

fn wait_for_usb_transport_core() {
    const USB_CORE_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let start = time_manager().uptime();
    while !usb_core_ready() {
        if time_manager().uptime().saturating_sub(start) >= USB_CORE_BOOT_TIMEOUT {
            panic!("timed out waiting for CPU2 USB transport worker");
        }
        spin_loop();
    }

    info!("CPU2 USB transport worker is ready");
}

fn bring_up_secondary_cpu(raw_id: u8) {
    const SECONDARY_BOOT_TIMEOUT: Duration = Duration::from_millis(100);

    let cpu = CpuId::new(raw_id).expect("BCM2711 secondary CPU ID must be valid");
    info!("Releasing {} for secondary EL1 bring-up", cpu);

    assert!(
        cpu::release_secondary(cpu),
        "refused to release secondary CPU"
    );

    let start = time_manager().uptime();

    loop {
        let status = cpu::secondary_status(cpu);

        if status.online {
            info!(
                "{} online: EL1={}, MMU={}, D-cache={}",
                cpu, status.at_el1, status.mmu_enabled, status.data_cache_enabled
            );

            assert!(status.at_el1, "{} did not enter EL1", cpu);
            assert!(status.mmu_enabled, "{} failed to enable its EL1 MMU", cpu);
            assert!(
                status.data_cache_enabled,
                "{} failed to enable its EL1 data cache",
                cpu
            );
            return;
        }

        if time_manager().uptime().saturating_sub(start) >= SECONDARY_BOOT_TIMEOUT {
            panic!("timed out waiting for {} secondary bring-up", cpu);
        }

        spin_loop();
    }
}

fn log_memory_summary() {
    const MIB: u64 = 1024 * 1024;

    let map = MEMORY_MANAGER.get().map();
    let tracked = map.total_bytes();
    let usable = map.bytes_of_kind(MemoryRegionKind::Usable);
    let reserved = map.bytes_of_kind(MemoryRegionKind::Reserved);
    let dma = map.bytes_of_kind(MemoryRegionKind::Dma);
    let no_map = map.bytes_of_kind(MemoryRegionKind::NoMap);

    info!(
        "Memory map: tracked={} MiB, usable={} MiB, reserved={} MiB, DMA={} MiB, no-map={} MiB",
        tracked / MIB,
        usable / MIB,
        reserved / MIB,
        dma / MIB,
        no_map / MIB,
    );

    PHYS_ALLOCATOR.with_ref(|phys| {
        if let (Some((arena_start, arena_end, arena_bytes)), Some(free_bytes)) =
            (phys.info(), phys.free_bytes())
        {
            info!(
                "Physical allocator: arena={:#018x}..{:#018x} ({} MiB), free={} MiB",
                arena_start,
                arena_end,
                arena_bytes / MIB,
                free_bytes / MIB,
            );
        }
    });

    KERNEL_HEAP.with_ref(|heap| {
        if let (Some(capacity), Some(used), Some(free)) =
            (heap.capacity_bytes(), heap.used_bytes(), heap.free_bytes())
        {
            info!(
                "Kernel heap: capacity={} MiB, used={} KiB, free={} KiB",
                (capacity as u64) / MIB,
                used / 1024,
                free / 1024,
            );
        }
    });
}

#[inline(always)]
pub fn read_daif() -> u64 {
    let value: u64;

    unsafe {
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}
