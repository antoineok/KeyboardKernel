mod panic_logger;

use crate::error;
use crate::panic::panic_logger::early_aux_print_u32;
use core::alloc::Layout;
use core::panic::PanicInfo;
use panic_logger::early_aux_write_str;

use core::sync::atomic::{AtomicBool, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if PANICKING.swap(true, Ordering::SeqCst) {
        loop {}
    }
    let uart = crate::drivers::uart::UART.get();
    if uart.is_initialized() {
        error!("Kernel Panic!");
        if let Some(msg) = info.message().as_str() {
            error!("Message: {}", msg);
        } else {
            error!("No message");
        }
        if let Some(loc) = info.location() {
            error!(
                "Location in file {} at {}:{}",
                loc.file(),
                loc.line(),
                loc.column()
            );
        }
    } else {
        early_aux_write_str("Kernel Panic!\n");
        early_aux_write_str("Message: ");
        if let Some(msg) = info.message().as_str() {
            early_aux_write_str(msg);
            early_aux_write_str("\n");
        } else {
            early_aux_write_str("No message\n");
        }

        if let Some(loc) = info.location() {
            early_aux_write_str("Location in file ");
            early_aux_write_str(loc.file());
            early_aux_write_str(" at ");
            early_aux_print_u32(loc.line());
            early_aux_write_str(":");
            early_aux_print_u32(loc.column());
            early_aux_write_str("\n");
        }
    }

    loop {
        unsafe { core::arch::asm!("wfe") }
    }
}

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    // You can log via UART and then panic.
    error!("Allocation error!");
    panic!("allocation error") // or whatever your panic path is
}
