#![no_main]
#![no_std]

use crate::{io::uart::UART, time::time_manager};

extern crate alloc;

mod cpu;
mod exceptions;
mod graphics;
mod io;
mod memory;
mod panic;
mod time;
mod util;

#[no_mangle]
pub extern "C" fn main(dtb_addr: usize) -> ! {
    let uart = UART.get();
    uart.init();
    info!("Logger initialized");

    info!("DTB address: {:#x}", dtb_addr);

    let (_, privilege_level) = exceptions::current_privilege_level();
    info!("Current privilege level: {}", privilege_level);

    info!("Exception handling state:");
    exceptions::asynchronous::print_state();



    info!("Timer test, spinning for 1 second");
    time_manager().spin_for(core::time::Duration::from_secs(1));
    info!("1 second elapsed");

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
