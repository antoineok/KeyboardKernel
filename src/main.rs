#![no_main]
#![no_std]

mod io;
mod panic;

use core::arch::global_asm;

global_asm!(include_str!("cpu/boot.s"));

#[no_mangle]
pub extern "C" fn main() -> ! {
    loop {}
}
