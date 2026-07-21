#![no_main]
#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;
mod arch;
mod drivers;
mod fdt;
mod graphics;
#[cfg(feature = "jtag")]
mod jtag;
mod kernel;
mod memory;
mod panic;
mod reader;
mod storage;
mod time;
mod update;
mod usb;
mod util;
