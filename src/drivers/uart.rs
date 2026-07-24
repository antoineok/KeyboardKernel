use super::{
    address::Address,
    gpio::{GpioFunction, GpioPin, GpioPull},
};

use core::{
    cell::UnsafeCell,
    hint::spin_loop,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

const AUX_UART_CLOCK: u32 = 500_000_000;
// The Cortex-A72 bring-up deliberately keeps SCTLR_EL1.C clear for now.
// Avoid LL/SC-based atomic RMW operations (LDXR/STXR) in the logger until
// the cache/coherency work is complete. Lamport's bakery algorithm gives us
// mutual exclusion using only atomic loads and stores.
//
// Keep these arrays explicit: Atomic* values are not Copy, and BCM2711 has
// exactly four Cortex-A72 cores.
static UART_LOG_CHOOSING: [AtomicBool; crate::arch::cpu::CPU_COUNT] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];
static UART_LOG_TICKET: [AtomicU64; crate::arch::cpu::CPU_COUNT] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

struct UartLogGuard {
    cpu: usize,
}

impl UartLogGuard {
    fn acquire() -> Self {
        let cpu = crate::arch::cpu::current_id().index();

        UART_LOG_CHOOSING[cpu].store(true, Ordering::SeqCst);

        let mut max_ticket = 0u64;
        for ticket in &UART_LOG_TICKET {
            max_ticket = max_ticket.max(ticket.load(Ordering::SeqCst));
        }

        let my_ticket = max_ticket.saturating_add(1);
        UART_LOG_TICKET[cpu].store(my_ticket, Ordering::SeqCst);
        UART_LOG_CHOOSING[cpu].store(false, Ordering::SeqCst);

        for other in 0..crate::arch::cpu::CPU_COUNT {
            if other == cpu {
                continue;
            }

            while UART_LOG_CHOOSING[other].load(Ordering::SeqCst) {
                spin_loop();
            }

            loop {
                let other_ticket = UART_LOG_TICKET[other].load(Ordering::SeqCst);
                if other_ticket == 0 || (other_ticket, other) > (my_ticket, cpu) {
                    break;
                }
                spin_loop();
            }
        }

        Self { cpu }
    }
}

impl Drop for UartLogGuard {
    fn drop(&mut self) {
        UART_LOG_TICKET[self.cpu].store(0, Ordering::SeqCst);
    }
}

fn aux_mu_baud(baud: u32) -> u32 {
    (AUX_UART_CLOCK / (baud * 8)) - 1
}

pub struct Uart {
    initialized: bool,
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
        let base_addr = mmio_entry
            .unwrap_or_else(|| panic!("UART MMIO entry not found in device tree"))
            .arm_phys;
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

    pub fn write_byte(&self, byte: u8) {
        while !self.is_write_ready() {}
        self.aux_mu_io_reg.write(byte as u32);
    }

    pub fn write_text(&self, text: &str) {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
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

    // Before EL1 enables the MMU only CPU0 can execute Rust, so there is no
    // concurrent logger to synchronize with yet.
    if !crate::arch::mmu::is_enabled() {
        let mut writer = SharedUartWriter(UART.get());
        writer.write_fmt(args).unwrap();
        return;
    }

    let _guard = UartLogGuard::acquire();

    let mut writer = SharedUartWriter(UART.get());
    writer.write_fmt(args).unwrap();
}

struct SharedUartWriter(&'static Uart);

impl core::fmt::Write for SharedUartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_text(s);
        Ok(())
    }
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_text(s);
        Ok(())
    }
}
