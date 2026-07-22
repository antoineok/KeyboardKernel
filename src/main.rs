#![no_main]
#![no_std]
#![feature(alloc_error_handler)]

extern crate alloc;
mod arch;
mod drivers;
mod fdt;
mod graphics;
mod hid;
#[cfg(feature = "jtag")]
mod jtag;
mod kernel;
mod memory;
mod panic;
mod reader;
mod services;
mod storage;
mod time;
mod transport;
mod update;
mod usb;
mod util;
