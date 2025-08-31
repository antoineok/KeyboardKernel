#![no_main]
#![no_std]
#![feature(core_intrinsics)]

use crate::{io::uart::UART, memory::mmu::interface::MMU, time::time_manager};

mod cpu;
mod exceptions;
mod graphics;
mod io;
mod panic;
mod time;
mod memory;
mod util;

#[no_mangle]
pub extern "C" fn main(dtb_addr: usize) -> ! {
    unsafe { exceptions::handling_init() };
    if let Err(string) = unsafe { memory::mmu::mmu().enable_mmu_and_caching() } {
        panic!("MMU: {}", string);
    }

    let uart = UART.get();
    uart.init();
    info!("Logger initialized");

    info!("MMU online. Special regions:");
    crate::memory::mmu::bsp_mmu::virt_mem_layout().print_layout();

    let (_, privilege_level) = exceptions::current_privilege_level();
    info!("Current privilege level: {}", privilege_level);



    info!("DTB address: {:#x}", dtb_addr);

    //let (_, privilege_level) = exceptions::current_privilege_level();
    //info!("Current privilege level: {}", privilege_level);

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
