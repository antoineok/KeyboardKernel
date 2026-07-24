pub mod panic_logger;

use crate::panic::panic_logger::early_aux_print_u32;
use crate::panic::panic_logger::early_aux_print_usize;
use crate::panic::panic_logger::early_aux_write_fmt;
use core::alloc::Layout;
use core::panic::PanicInfo;
use panic_logger::early_aux_write_str;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
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

    #[cfg(feature = "jtag")]
    crate::jtag::breakpoint();

    panic_halt()
}

#[inline(never)]
fn panic_halt() -> ! {
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }

    loop {
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack));
        }
    }
}

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    early_aux_write_str("\n=== ALLOCATION FAILURE ===\n");
    early_aux_write_str("Size: ");
    early_aux_print_usize(layout.size());
    early_aux_write_str("\nAlignment: ");
    early_aux_print_usize(layout.align());
    early_aux_write_str("\n");

    #[cfg(feature = "jtag")]
    crate::jtag::breakpoint();

    panic_halt()
}
