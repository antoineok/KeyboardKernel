use core::arch::global_asm;

global_asm!(include_str!("jtag.s"));

extern "C" {
    fn wait_for_debugger();
}

#[no_mangle]
pub extern "C" fn breakpoint() {
    unsafe { core::arch::asm!("brk 0", options(nomem, nostack)) }
}

pub fn init_jtag() {
    use crate::io::gpio::GpioPin;
    GpioPin::new(22).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO22 = ARM_TRST
    GpioPin::new(23).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO23 = ARM_RTCK
    GpioPin::new(24).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO24 = ARM_TDO
    GpioPin::new(25).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO25 = ARM_TCK
    GpioPin::new(26).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO26 = ARM_TDI
    GpioPin::new(27).set_function(crate::io::gpio::GpioFunction::Alt4); // GPIO27 = ARM_TMS

    crate::info!("Waiting for debugger to attach...");

    unsafe { wait_for_debugger() };

    crate::info!("Debugger attached, continuing boot");
}
