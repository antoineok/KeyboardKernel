use crate::{exceptions, info, io::uart::UART};
use core::arch::global_asm;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
global_asm!(
    "
    .global park
    park:
        wfi
        b park
"
);
extern "C" {
    fn el1_drop();
    fn park() -> !;
}

pub static DTB_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[no_mangle]
pub extern "C" fn el2_start(device_tree_ptr: usize) -> ! {
    let uart = UART.get();
    uart.init();
    info!("Logger initialized");

    info!("device_tree_ptr: {:#x}", device_tree_ptr);

    #[cfg(feature = "jtag")]
    {
        crate::jtag::init_jtag();
        crate::info!("JTAG initialized");
    }
    DTB_ADDR.store(device_tree_ptr, core::sync::atomic::Ordering::Relaxed);
    info!(
        "DTB address: {:#x}",
        DTB_ADDR.load(core::sync::atomic::Ordering::Relaxed)
    );

    let (_, privilege_level) = exceptions::current_privilege_level();
    info!("Current privilege level: {}", privilege_level);

    unsafe {
        el1_drop();

        park()
    }
}
