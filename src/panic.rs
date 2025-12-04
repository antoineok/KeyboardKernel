use crate::error;
use core::alloc::Layout;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("Kernel Panic!");
    error!("Message: {}", info.message());
    if let Some(loc) = info.location() {
        error!(
            "Location in file {} at {}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        );
    }
    loop {}
}

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    // You can log via UART and then panic.
    error!("Allocation error!");
    panic!("allocation error") // or whatever your panic path is
}
