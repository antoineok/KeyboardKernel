use aarch64_cpu::asm::nop;

// Global flag that GDB will poke.
#[no_mangle]
pub static mut DEBUGGER_FLAG: u32 = 0;

/// Spin until the debugger sets `DEBUGGER_FLAG` to 1.
#[inline(never)]
pub unsafe fn wait_for_debugger() {
    while core::ptr::read_volatile(&raw const DEBUGGER_FLAG) == 0 {
        // Prevent aggressive optimization and yield the pipeline a bit.
        nop();
    }
    core::ptr::write_volatile(&raw mut DEBUGGER_FLAG, 0); // reset for next time
}

#[no_mangle]
#[inline(never)] // Prevent inlining to ensure a stable breakpoint location
pub extern "C" fn breakpoint() { /* nothing */
    unsafe {
        core::arch::asm!(
            "nop",
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub fn init_jtag() {
    use crate::drivers::gpio::GpioPin;
    GpioPin::new(22).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO22 = ARM_TRST
    GpioPin::new(23).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO23 = ARM_RTCK
    GpioPin::new(24).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO24 = ARM_TDO
    GpioPin::new(25).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO25 = ARM_TCK
    GpioPin::new(26).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO26 = ARM_TDI
    GpioPin::new(27).set_function(crate::drivers::gpio::GpioFunction::Alt4); // GPIO27 = ARM_TMS

    crate::info!("Waiting for debugger to attach...");

    unsafe {
        core::ptr::write_volatile(&raw mut DEBUGGER_FLAG, 0);

        crate::info!(
            "current DEBUGGER_FLAG value: {}",
            core::ptr::read_volatile(&raw const DEBUGGER_FLAG)
        );

        wait_for_debugger();

        crate::info!(
            "DEBUGGER_FLAG new value: {}",
            core::ptr::read_volatile(&raw const DEBUGGER_FLAG)
        );
    }

    crate::info!("Debugger attached, continuing boot");
}
