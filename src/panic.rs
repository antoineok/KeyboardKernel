use core::{panic::PanicInfo};

use crate::error;

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
