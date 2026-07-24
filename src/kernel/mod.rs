pub mod init;

use aarch64_cpu::asm::nop;
use core::{hint::spin_loop, time::Duration};

use crate::arch::{
    cpu::{self, CpuId},
    exceptions::PrivilegeLevel,
};

use crate::assets::ImageAsset;
use crate::drivers::{
    gpio::{GpioPin, GpioPull},
    uart::UART,
};
use crate::graphics::framebuffer::FRAMEBUFFER;
use crate::graphics::{BlendMode, Compositor, ImageLayer, Layer, Point, SourceRect};
use crate::hid::Key;
use crate::memory::{heap::KERNEL_HEAP, phys::PHYS_ALLOCATOR, MemoryRegionKind, MEMORY_MANAGER};
use crate::storage::fs::vfs::{FsId, VFS};
use crate::time::time_manager;

use crate::{
    early_timeit,
    hid::{KeyboardModifiers, KeyboardReport},
    info,
    services::{KernelManagementBackend, KeyworkServices},
    transport::{KeyworkTransport, TransportState},
    usb::{MANAGEMENT_FRAME_CAPACITY, USB_TRANSPORT},
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
    bring_up_secondary_cpu(2);

    info!("Timer test, spinning for 1 second");
    time_manager().spin_for(core::time::Duration::from_secs(1));
    info!("1 second elapsed");

    let usb_transport = unsafe { USB_TRANSPORT.get_mut() };
    usb_transport
        .initialize()
        .expect("USB transport initialization failed");

    let mut services = KeyworkServices::<MANAGEMENT_FRAME_CAPACITY>::new();
    let mut management_backend = KernelManagementBackend::new();

    // Temporary HID test input. GPIO17 is held high by its internal pull-up.
    // Connecting GPIO17 directly to GND presses A; disconnecting it releases A.
    const HID_TEST_GPIO: u32 = 17;
    const HID_TEST_DEBOUNCE: Duration = Duration::from_millis(5);

    let hid_test_pin = GpioPin::new(HID_TEST_GPIO);
    hid_test_pin.configure_input(GpioPull::Up);

    let mut hid_test_sample = hid_test_pin.is_low();
    let mut hid_test_stable = hid_test_sample;
    let mut hid_test_sample_since = time_manager().uptime();

    if hid_test_stable {
        services.hid_mut().set_keyboard(KeyboardReport {
            modifiers: KeyboardModifiers::LEFT_SHIFT,
            keys: [
                Key::F,
                Key::None,
                Key::None,
                Key::None,
                Key::None,
                Key::None,
            ],
        });
    }

    info!(
        "HID GPIO test enabled on GPIO{}: connect pin to GND to hold A",
        HID_TEST_GPIO
    );

    let vfs = VFS.get();
    let background_file = vfs
        .open_fs(FsId::Rom, "background.kwim")
        .expect("failed to load background image");
    let background =
        ImageAsset::parse(background_file.as_bytes()).expect("failed to parse background image");
    let background_surface = background.surface();

    let compositor = Compositor::new(0x000000ff);
    let layers = [Layer::Image(ImageLayer {
        source: background_surface,
        source_rect: SourceRect::whole(&background_surface),
        position: Point::new(0, 0),
        blend_mode: BlendMode::Opaque,
    })];
    let framebuffer = FRAMEBUFFER.get();

    {
        let mut backbuffer = framebuffer
            .as_surface_mut()
            .expect("framebuffer backbuffer unavailable");

        compositor
            .render(&mut backbuffer, &layers)
            .expect("frame composition failed");
    }

    assert!(framebuffer.present());

    log_memory_summary();

    loop {
        let now = time_manager().uptime();
        let sample = hid_test_pin.is_low();

        if sample != hid_test_sample {
            hid_test_sample = sample;
            hid_test_sample_since = now;
        }

        if hid_test_sample != hid_test_stable
            && now.saturating_sub(hid_test_sample_since) >= HID_TEST_DEBOUNCE
        {
            hid_test_stable = hid_test_sample;

            let report = if hid_test_stable {
                KeyboardReport {
                    modifiers: KeyboardModifiers::LEFT_SHIFT,
                    keys: [
                        Key::F,
                        Key::None,
                        Key::None,
                        Key::None,
                        Key::None,
                        Key::None,
                    ],
                }
            } else {
                KeyboardReport::released()
            };

            services.hid_mut().set_keyboard(report);
            info!(
                "HID GPIO test: F {}",
                if hid_test_stable {
                    "pressed"
                } else {
                    "released"
                }
            );
        }

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
            .poll_selected_transport(usb_transport, &mut management_backend)
            .expect("USB service polling failed");
        management_backend.poll_pending_action();

        let updater = crate::update::uart::UART_UPDATE.get_mut();

        updater.poll();

        if updater.is_binary_mode() {
            continue;
        }

        UART.get_mut().flush_tx();
        nop();
    }
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
                "{} online: EL1={}, MMU={}",
                cpu, status.at_el1, status.mmu_enabled
            );

            assert!(status.at_el1, "{} did not enter EL1", cpu);
            assert!(status.mmu_enabled, "{} failed to enable its EL1 MMU", cpu);
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

    let phys = PHYS_ALLOCATOR.get();
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

    let heap = KERNEL_HEAP.get();
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
