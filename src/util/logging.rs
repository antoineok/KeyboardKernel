use crate::{io::uart::{UART}, uart_println};


pub static LOGGER: Logger = Logger::new();

pub struct Logger;

impl Logger{

    const fn new() -> Self {
        Logger {}
    }

    fn init(&self) {

    let uart = UART.get();
    uart.init();
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            uart_println!("{} - {}\n", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}