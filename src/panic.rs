pub mod panic_logger;

use crate::panic::panic_logger::early_aux_print_u32;
use crate::{error, panic::panic_logger::early_aux_write_fmt};
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
    early_aux_write_str("Kernel Panic!\n");
    early_aux_write_str("Message: ");
    early_aux_write_fmt(format_args!("{}", info.message()));
    early_aux_write_str("\n");

    if let Some(loc) = info.location() {
        early_aux_write_str("Location in file ");
        early_aux_write_str(loc.file());
        early_aux_write_str(" at ");
        early_aux_print_u32(loc.line());
        early_aux_write_str(":");
        early_aux_print_u32(loc.column());
        early_aux_write_str("\n");
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
