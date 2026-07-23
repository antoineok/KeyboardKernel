pub mod init;

use aarch64_cpu::asm::nop;
use core::time::Duration;

use crate::arch::exceptions::PrivilegeLevel;

use crate::drivers::{
    gpio::{GpioPin, GpioPull},
    uart::UART,
};
use crate::hid::Key;
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
