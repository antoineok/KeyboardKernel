#![no_main]
#![no_std]

mod cpu;
mod graphics;
mod io;
mod panic;

use crate::io::uart::UART;

#[no_mangle]
pub extern "C" fn main() -> ! {
    let uart = UART.get();
    uart.init();
    let framebuffer = graphics::framebuffer::FRAMEBUFFER.get();
    framebuffer.init();
    uart_println!("Hello, World!");
    framebuffer.draw_rect(150, 150, 400, 400, 0x00FFFF, false); //cyan
    framebuffer.draw_rect(200, 200, 300, 300, 0xFF00FF, true); //magenta

    framebuffer.draw_circle(960, 540, 250, 0xFFFFFF, false);
    framebuffer.draw_circle(960, 540, 50, 0xADD8E6, true); //light blue

    framebuffer.set_pixel(250, 250, 0xFFFFFF);

    framebuffer.draw_line(100, 500, 350, 700, 0xFF0000);
    loop {
        uart.update();
        // Add any other functionality or loop logic here
    }
}
