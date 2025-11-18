#![no_main]
#![no_std]

use aarch64_cpu::asm::nop;

use crate::{cpu::DTB_ADDR, time::time_manager};

#[cfg(feature = "jtag")]
use crate::jtag::breakpoint;

extern crate alloc;

mod cpu;
mod exceptions;
mod graphics;
mod io;
#[cfg(feature = "jtag")]
mod jtag;
mod memory;
mod panic;
mod time;
mod util;

#[no_mangle]
pub extern "C" fn main() -> ! {
    dump_el1_regs();

    #[cfg(feature = "jtag")]
    breakpoint();

    let (_, privilege_level) = exceptions::current_privilege_level();
    info!("Current privilege level: {}", privilege_level);

    info!("Exception handling state:");
    exceptions::asynchronous::print_state();

    info!("Timer test, spinning for 1 second");
    time_manager().spin_for(core::time::Duration::from_secs(1));
    info!("1 second elapsed");

    let fdt = unsafe {
        fdt::Fdt::from_ptr(DTB_ADDR.load(core::sync::atomic::Ordering::Relaxed) as *const u8)
            .unwrap()
    };
    info!(
        "This is a devicetree representation of a {}",
        fdt.root().model()
    );
    info!(
        "...which is compatible with at least: {}",
        fdt.root().compatible().first()
    );
    info!("...and has {} CPU(s)", fdt.cpus().count());
    info!(
        "...and has at least one memory location at: {:#X}\n",
        fdt.memory().regions().next().unwrap().starting_address as usize
    );

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
        info!("Idle loop iteration");
        time_manager().spin_for(core::time::Duration::from_secs(5));
        nop();
    }
}

pub fn dump_el1_regs() {
    let mut el: u64;
    let mut sctlr: u64;
    let mut vbar: u64;
    unsafe {
        core::arch::asm!("mrs {0}, CurrentEL", out(reg) el);
        core::arch::asm!("mrs {0}, SCTLR_EL1", out(reg) sctlr);
        core::arch::asm!("mrs {0}, VBAR_EL1", out(reg) vbar);
    }
    crate::info!(
        "CurrentEL = {:#x}, SCTLR_EL1 = {:#x}, VBAR_EL1 = {:#x}",
        el,
        sctlr,
        vbar
    );
}
