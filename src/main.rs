#![no_main]
#![no_std]

mod io;
mod panic;

use core::arch::global_asm;

use crate::io::uart::UART;

global_asm!(include_str!("cpu/boot.s"));

#[no_mangle]
pub extern "C" fn main() -> ! {
    let uart = UART.get();
    uart.init();
    uart_println!("Hello, World!");
    uart_println!("Ready to cause a kernel panic!");
    Option::<()>::None.unwrap(); // This will cause a panic to test the panic handler
    loop {
        uart.update();
        // Add any other functionality or loop logic here
    }
}
