use super::{
    address::Address,
    gpio::{GpioFunction, GpioPin, GpioPull},
};

use core::cell::UnsafeCell;

const AUX_UART_CLOCK: u32 = 500_000_000;
const UART_MAX_QUEUE: usize = 16 * 1024;

fn aux_mu_baud(baud: u32) -> u32 {
    (AUX_UART_CLOCK / (baud * 8)) - 1
}

pub struct Uart {
    initialized: bool,
    output_queue: [u8; UART_MAX_QUEUE],
    output_queue_write: usize,
    output_queue_read: usize,
    aux_base: Address,        // = MmioAddress(PERIPHERAL_BASE + 0x215000);
    aux_enable: Address,      // = aux_base.offset(4);
    aux_mu_io_reg: Address,   // = aux_base.offset(64);
    aux_mu_ier_reg: Address,  // = aux_base.offset(68);
    aux_mu_iir_reg: Address,  // = aux_base.offset(72);
    aux_mu_lcr_reg: Address,  // = aux_base.offset(76);
    aux_mu_mcr_reg: Address,  // = aux_base.offset(80);
    aux_mu_lsr_reg: Address,  // = aux_base.offset(84);
    aux_mu_cntl_reg: Address, // = aux_base.offset(96);
    aux_mu_baud_reg: Address, // = aux_base.offset(104);
}
pub struct UartStatic(UnsafeCell<Uart>);

unsafe impl Sync for UartStatic {}

pub static UART: UartStatic = UartStatic(UnsafeCell::new(Uart::new()));

impl UartStatic {
    pub fn get_mut(&self) -> &mut Uart {
        unsafe { &mut *self.0.get() }
    }

    pub fn get(&self) -> &Uart {
        unsafe { &*self.0.get() }
    }
}

impl Uart {
    pub const fn new() -> Self {
        Uart {
            initialized: false,
            output_queue: [0; UART_MAX_QUEUE],
            output_queue_write: 0,
            output_queue_read: 0,
            aux_base: Address(0),
            aux_enable: Address(0),
            aux_mu_io_reg: Address(0),
            aux_mu_ier_reg: Address(0),
            aux_mu_iir_reg: Address(0),
            aux_mu_lcr_reg: Address(0),
            aux_mu_mcr_reg: Address(0),
            aux_mu_lsr_reg: Address(0),
            aux_mu_cntl_reg: Address(0),
            aux_mu_baud_reg: Address(0),
        }
    }

    pub fn init(&mut self, mmio_entry: Option<crate::fdt::AliasEntry>) {
        let base_addr = match mmio_entry {
            Some(entry) => entry.arm_phys,
            None => panic!("UART MMIO entry not found in device tree"),
        };
        // Initialize MMIO addresses
        self.aux_base.set_address(base_addr);
        self.aux_enable = self.aux_base.offset(4);
        self.aux_mu_io_reg = self.aux_base.offset(64);
        self.aux_mu_ier_reg = self.aux_base.offset(68);
        self.aux_mu_iir_reg = self.aux_base.offset(72);
        self.aux_mu_lcr_reg = self.aux_base.offset(76);
        self.aux_mu_mcr_reg = self.aux_base.offset(80);
        self.aux_mu_lsr_reg = self.aux_base.offset(84);
        self.aux_mu_cntl_reg = self.aux_base.offset(96);
        self.aux_mu_baud_reg = self.aux_base.offset(104);

        // Enable Mini UART
        self.aux_enable.write(1);
        // Disable interrupts & control lines, clear FIFO
        self.aux_mu_ier_reg.write(0);
        self.aux_mu_cntl_reg.write(0);
        self.aux_mu_lcr_reg.write(3); // 8 bits
        self.aux_mu_mcr_reg.write(0);
        self.aux_mu_iir_reg.write(0xC6); // Disable interrupts

        // Set baud rate
        self.aux_mu_baud_reg.write(aux_mu_baud(115200));

        // Set GPIO14 and GPIO15 to ALT5 (TXD1/RXD1), no pull-up/down
        let tx = GpioPin::new(14);
        let rx = GpioPin::new(15);
        tx.set_pull(GpioPull::None);
        tx.set_function(GpioFunction::Alt5);
        rx.set_pull(GpioPull::None);
        rx.set_function(GpioFunction::Alt5);

        // Enable transmitter and receiver
        self.aux_mu_cntl_reg.write(3);
        self.initialized = true;
    }

    fn is_write_ready(&self) -> bool {
        self.aux_mu_lsr_reg.read() & 0x20 != 0
    }

    fn is_read_ready(&self) -> bool {
        self.aux_mu_lsr_reg.read() & 0x01 != 0
    }

    pub fn write_byte(&self, byte: u8) {
        while !self.is_write_ready() {}
        self.aux_mu_io_reg.write(byte as u32);
    }

    pub fn read_byte(&self) -> u8 {
        while !self.is_read_ready() {}
        (self.aux_mu_io_reg.read() & 0xFF) as u8
    }

    pub fn try_read_byte(&self) -> Option<u8> {
        if self.is_read_ready() {
            Some((self.aux_mu_io_reg.read() & 0xFF) as u8)
        } else {
            None
        }
    }

    pub fn flush_tx(&mut self) {
        self.load_output_fifo();
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
            if byte == b'\r' {
                self.write_text("\n");
            } else {
                self.write_blocking_byte(byte);
            }
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

unsafe impl Sync for Uart {}

#[macro_export]
macro_rules! uart_print {
    ($($arg:tt)*) => ($crate::drivers::uart::_uart_print(format_args!($($arg)*)));
}

//macro for println!
#[macro_export]
macro_rules! uart_println {
    () => ($crate::uart_print!("\n"));
    ($($arg:tt)*) => ($crate::uart_print!("{}\n", format_args!($($arg)*)));
}

pub fn _uart_print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let uart = UART.get_mut();
    uart.write_fmt(args).unwrap();
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_text(s);
        Ok(())
    }
}
