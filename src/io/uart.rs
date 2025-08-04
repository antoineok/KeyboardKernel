use super::{
    gpio::{GpioFunction, GpioPin, GpioPull},
    mmio::MmioAddress,
    PERIPHERAL_BASE,
};

use core::cell::UnsafeCell;

const AUX_BASE: MmioAddress = MmioAddress(PERIPHERAL_BASE + 0x215000);
const AUX_ENABLES: MmioAddress = AUX_BASE.offset(4);
const AUX_MU_IO_REG: MmioAddress = AUX_BASE.offset(64);
const AUX_MU_IER_REG: MmioAddress = AUX_BASE.offset(68);
const AUX_MU_IIR_REG: MmioAddress = AUX_BASE.offset(72);
const AUX_MU_LCR_REG: MmioAddress = AUX_BASE.offset(76);
const AUX_MU_MCR_REG: MmioAddress = AUX_BASE.offset(80);
const AUX_MU_LSR_REG: MmioAddress = AUX_BASE.offset(84);
const AUX_MU_CNTL_REG: MmioAddress = AUX_BASE.offset(96);
const AUX_MU_BAUD_REG: MmioAddress = AUX_BASE.offset(104);
const AUX_UART_CLOCK: u32 = 500_000_000;
const UART_MAX_QUEUE: usize = 16 * 1024;

fn aux_mu_baud(baud: u32) -> u32 {
    (AUX_UART_CLOCK / (baud * 8)) - 1
}

pub struct Uart {
    output_queue: [u8; UART_MAX_QUEUE],
    output_queue_write: usize,
    output_queue_read: usize,
}
pub struct UartStatic(UnsafeCell<Uart>);

unsafe impl Sync for UartStatic {}

pub static UART: UartStatic = UartStatic(UnsafeCell::new(Uart::new()));

impl UartStatic {
    pub fn get(&self) -> &mut Uart {
        unsafe { &mut *self.0.get() }
    }
}

impl Uart {
    pub const fn new() -> Self {
        Uart {
            output_queue: [0; UART_MAX_QUEUE],
            output_queue_write: 0,
            output_queue_read: 0,
        }
    }

    pub fn init(&self) {
        // Enable Mini UART
        AUX_ENABLES.write(1);

        // Disable interrupts & control lines, clear FIFO
        AUX_MU_IER_REG.write(0);
        AUX_MU_CNTL_REG.write(0);
        AUX_MU_LCR_REG.write(3); // 8 bits
        AUX_MU_MCR_REG.write(0);
        AUX_MU_IIR_REG.write(0xC6); // Disable interrupts

        // Set baud rate
        AUX_MU_BAUD_REG.write(aux_mu_baud(115200));

        // Set GPIO14 and GPIO15 to ALT5 (TXD1/RXD1), no pull-up/down
        let tx = GpioPin::new(14);
        let rx = GpioPin::new(15);
        tx.set_pull(GpioPull::None);
        tx.set_function(GpioFunction::Alt5);
        rx.set_pull(GpioPull::None);
        rx.set_function(GpioFunction::Alt5);

        // Enable transmitter and receiver
        AUX_MU_CNTL_REG.write(3);
    }

    fn is_write_ready(&self) -> bool {
        AUX_MU_LSR_REG.read() & 0x20 != 0
    }

    fn is_read_ready(&self) -> bool {
        AUX_MU_LSR_REG.read() & 0x01 != 0
    }

    pub fn write_byte(&self, byte: u8) {
        while !self.is_write_ready() {}
        AUX_MU_IO_REG.write(byte as u32);
    }

    pub fn read_byte(&self) -> u8 {
        while !self.is_read_ready() {}
        (AUX_MU_IO_REG.read() & 0xFF) as u8
    }

    pub fn write_text(&self, text: &str) {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    fn is_output_queue_empty(&self) -> bool {
        self.output_queue_write == self.output_queue_read
    }

    fn write_blocking_byte(&mut self, byte: u8) {
        let next = (self.output_queue_write + 1) & (UART_MAX_QUEUE - 1);
        while next == self.output_queue_read {
            self.load_output_fifo();
        }
        self.output_queue[self.output_queue_write] = byte;
        self.output_queue_write = next;
    }

    fn load_output_fifo(&mut self) {
        while !self.is_output_queue_empty() && self.is_write_ready() {
            let byte = self.output_queue[self.output_queue_read];
            self.output_queue_read = (self.output_queue_read + 1) & (UART_MAX_QUEUE - 1);
            self.write_blocking_byte(byte);
        }
    }

    pub fn update(&mut self) {
        self.load_output_fifo();
        if self.is_read_ready() {
            let byte = self.read_byte();
            if (byte == b'\r') {
                self.write_text("\n");
            } else {
                self.write_blocking_byte(byte);
            }
        }
    }
}

unsafe impl Sync for Uart {}

#[macro_export]
macro_rules! uart_print {
    ($($arg:tt)*) => ($crate::io::uart::_uart_print(format_args!($($arg)*)));
}

//macro for println!
#[macro_export]
macro_rules! uart_println {
    () => ($crate::uart_print!("\n"));
    ($($arg:tt)*) => ($crate::uart_print!("{}\n", format_args!($($arg)*)));
}

pub fn _uart_print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let uart = UART.get();
    uart.write_fmt(args).unwrap();
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_text(s);
        Ok(())
    }
}
