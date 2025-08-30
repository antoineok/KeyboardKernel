#![no_main]
#![no_std]

extern crate alloc;

mod cpu;
mod graphics;
mod io;
mod memory;
mod panic;
mod util;

use log::info;

use crate::util::logging::LOGGER;


#[no_mangle]
pub extern "C" fn main(dtb_addr: usize) -> ! {
    LOGGER.init();
    uart_println!("Logger initialized");
    log::set_logger(&LOGGER).inspect_err( |e| {
        panic!("Failed to set logger: {}", e);
    }).unwrap();

    info!("Logger initialized");
    //memory::memory_init(dtb_addr);

    let framebuffer = graphics::framebuffer::FRAMEBUFFER.get();
    framebuffer.init();
    framebuffer.draw_rect(150, 150, 400, 400, 0x00FFFF, false); //cyan
    framebuffer.draw_rect(200, 200, 300, 300, 0xFF00FF, true); //magenta

    framebuffer.draw_circle(960, 540, 250, 0xFFFFFF, false);
    framebuffer.draw_circle(960, 540, 50, 0xADD8E6, true); //light blue

    framebuffer.set_pixel(250, 250, 0xFFFFFF);

    framebuffer.draw_line(100, 500, 350, 700, 0xFF0000);
    loop {
        //uart.update();
        // Add any other functionality or loop logic here
    }
}
