#[inline(always)]
fn mmio_read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[inline(always)]
fn mmio_write(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

const PERI_BASE: usize = 0xFE00_0000;

// GPIO
const GPIO_BASE: usize = PERI_BASE + 0x200_000;
const GPFSEL1: usize = GPIO_BASE + 0x04;
const GPPUPPDN0: usize = GPIO_BASE + 0xE4; // Pi 4 / BCM2711 pull control

// AUX / miniUART
const AUX_BASE: usize = PERI_BASE + 0x215_000;
const AUX_ENABLES: usize = AUX_BASE + 0x04;

const AUX_MU_IO: usize = AUX_BASE + 0x40;
const AUX_MU_IER: usize = AUX_BASE + 0x44;
const AUX_MU_IIR: usize = AUX_BASE + 0x48;
const AUX_MU_LCR: usize = AUX_BASE + 0x4C;
const AUX_MU_MCR: usize = AUX_BASE + 0x50;
const AUX_MU_LSR: usize = AUX_BASE + 0x54;
const AUX_MU_CNTL: usize = AUX_BASE + 0x60;
const AUX_MU_BAUD: usize = AUX_BASE + 0x68;

fn aux_mu_baud(baud: u32) -> u32 {
    (500_000_000 / (baud * 8)) - 1
}

pub fn early_aux_uart_init() {
    // 1) Enable the AUX block (miniUART)
    mmio_write(AUX_ENABLES, 1);

    // 2) Disable UART while configuring
    mmio_write(AUX_MU_CNTL, 0);
    mmio_write(AUX_MU_IER, 0); // no interrupts
    mmio_write(AUX_MU_MCR, 0); // no RTS/CTS
    mmio_write(AUX_MU_LCR, 3); // 8-bit mode
    mmio_write(AUX_MU_IIR, 0xC6); // clear FIFOs (safe “quick” setting)

    // 3) GPIO14/15 -> ALT5 (TXD1/RXD1 for miniUART)
    let mut fsel1 = mmio_read(GPFSEL1);
    // GPIO14 (bits 12..14), GPIO15 (bits 15..17)
    fsel1 &= !((7 << 12) | (7 << 15));
    fsel1 |= (2 << 12) | (2 << 15); // ALT5 = 0b010
    mmio_write(GPFSEL1, fsel1);

    // 4) Disable pulls on GPIO14/15 (BCM2711 uses GPPUPPDN*)
    // Each GPIO has 2 bits in GPPUPPDN0: 00=none, 01=pull-up, 10=pull-down
    let mut pud = mmio_read(GPPUPPDN0);
    // GPIO14 -> bits 28..29, GPIO15 -> bits 30..31
    pud &= !((3 << 28) | (3 << 30));
    mmio_write(GPPUPPDN0, pud);

    // 5) Baud rate
    // Baud = uart_clk / (8*(BAUD+1))
    // If uart_clk is 250MHz, BAUD=270 ≈ 115200.
    // If your uart_clk differs, output will be garbage or silent.
    mmio_write(AUX_MU_BAUD, aux_mu_baud(115200));

    // 6) Enable transmitter (TX only)
    mmio_write(AUX_MU_CNTL, 2);
}

pub fn early_aux_putc(c: u8) {
    // wait for TX empty (LSR bit 5)
    while (mmio_read(AUX_MU_LSR) & (1 << 5)) == 0 {}
    mmio_write(AUX_MU_IO, c as u32);
}

pub fn early_aux_write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            early_aux_putc(b'\r');
        }
        early_aux_putc(b);
    }
}

pub fn early_aux_print_u32(mut value: u32) {
    // Special case for 0
    if value == 0 {
        early_aux_putc(b'0');
        return;
    }

    // u32 max is 10 digits
    let mut buf = [0u8; 10];
    let mut i = 0;

    while value > 0 {
        buf[i] = b'0' + (value % 10) as u8;
        value /= 10;
        i += 1;
    }

    // Digits are reversed
    while i > 0 {
        i -= 1;
        early_aux_putc(buf[i]);
    }
}

pub fn early_aux_print_hex_u32(value: u32) {
    early_aux_write_str("0x");

    for i in (0..8).rev() {
        let nibble = (value >> (i * 4)) & 0xF;
        let c = match nibble {
            0..=9 => b'0' + nibble as u8,
            _ => b'a' + (nibble as u8 - 10),
        };
        early_aux_putc(c);
    }
}
