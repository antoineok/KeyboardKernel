pub mod init;

use aarch64_cpu::asm::nop;

use crate::arch::exceptions::PrivilegeLevel;

use crate::drivers::uart::UART;
use crate::graphics::framebuffer::FRAMEBUFFER;
use crate::time::time_manager;

use crate::{early_timeit, info, timeit};

#[cfg(feature = "jtag")]
use crate::jtag::breakpoint;

#[no_mangle]
pub extern "C" fn el1_entry(device_tree_ptr: usize) -> ! {
    // We are now in EL1, with a basic stack (set by EL2 code).
    unsafe {
        early_timeit!(
            "Kernel early EL1 init",
            {init::early_el1_init(device_tree_ptr)}
        );

        crate::util::early_timing::flush_early_timings();
    }

    info!("DAIF = {:#x}", read_daif());

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
    timeit!("Framebuffer test", {
        framebuffer.clear(0xFFFFFFFF); //cyan
    });
    framebuffer.draw_rect(150, 150, 400, 400, 0x00FFFF, false); //cyan
    framebuffer.draw_rect(200, 200, 300, 300, 0xFF00FF, true); //magenta

    framebuffer.draw_circle(960, 540, 250, 0x000000FF, false);
    framebuffer.draw_circle(960, 540, 50, 0xADD8E6, true); //light blue

    framebuffer.set_pixel(250, 250, 0x000000FF);

    framebuffer.draw_line(100, 500, 350, 700, 0xFF0000);

    loop {
        let updater = crate::update::uart::UART_UPDATE.get_mut();

        updater.poll();

        if updater.is_binary_mode() {
            continue;
        }

        UART.get_mut().flush_tx();
        nop();
    }
}

#[inline(always)]
pub fn read_daif() -> u64 {
    let value: u64;

    unsafe {
        core::arch::asm!(
            "mrs {}, daif",
            out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }

    value
}