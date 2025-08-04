use core::panic::PanicInfo;

use crate::uart_println;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uart_println!("Kernel Panic!");
    uart_println!("Message: {}", info.message());
    if let Some(loc) = info.location() {
        uart_println!(
            "Location in file {} at {}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        );
    }
    loop {}
}
