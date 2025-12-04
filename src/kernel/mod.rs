pub mod init;

use aarch64_cpu::asm::nop;

use crate::arch::exceptions::PrivilegeLevel;

use crate::graphics::framebuffer::FRAMEBUFFER;
use crate::time::time_manager;

use crate::info;

#[cfg(feature = "jtag")]
use crate::jtag::breakpoint;

#[no_mangle]
pub extern "C" fn el1_entry() -> ! {
    // We are now in EL1, with a basic stack (set by EL2 code).
    unsafe {
        init::early_el1_init();
    }

    #[cfg(feature = "jtag")]
    breakpoint();

    info!(
        "Current privilege level: {}",
        PrivilegeLevel::current().as_str()
    );

    info!("Timer test, spinning for 1 second");
    time_manager().spin_for(core::time::Duration::from_secs(1));
    info!("1 second elapsed");

    let framebuffer = FRAMEBUFFER.get();
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
