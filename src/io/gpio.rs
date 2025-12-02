use super::mmio::MmioAddress;
use super::PERIPHERAL_BASE;

pub const GPIO_MAX_PIN: u32 = 53;

const GPFSEL0: MmioAddress = MmioAddress(PERIPHERAL_BASE + 0x200000);
const GPSET0: MmioAddress = GPFSEL0.offset(0x1C);
const GPCLR0: MmioAddress = GPFSEL0.offset(0x28);
const GPPUPPDN0: MmioAddress = GPFSEL0.offset(0xE4);

#[derive(Copy, Clone, Debug)]
pub enum GpioFunction {
    Input = 0b000,
    Output = 0b001,
    Alt0 = 0b100,
    Alt1 = 0b101,
    Alt2 = 0b110,
    Alt3 = 0b111,
    Alt4 = 0b011,
    Alt5 = 0b010,
}

#[derive(Copy, Clone, Debug)]
pub enum GpioPull {
    None = 0b00,
    Up = 0b01,
    Down = 0b10,
}

pub struct GpioPin {
    pin: u32,
}

impl GpioPin {
    pub const fn new(pin: u32) -> Self {
        GpioPin { pin }
    }

    pub fn set_function(&self, func: GpioFunction) {
        let reg = GPFSEL0.offset(((self.pin / 10) * 4) as usize);
        let shift = (self.pin % 10) * 3;
        let mut val = reg.read();
        val &= !(0b111 << shift);
        val |= (func as u32) << shift;
        reg.write(val);
    }

    pub fn set_pull(&self, pull: GpioPull) {
        let reg = GPPUPPDN0.offset(((self.pin / 16) * 4) as usize);
        let shift = (self.pin % 16) * 2;
        let mut val = reg.read();
        val &= !(0b11 << shift);
        val |= (pull as u32) << shift;
        reg.write(val);
    }

    pub fn set_high(&self) {
        let reg = GPSET0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }

    pub fn set_low(&self) {
        let reg = GPCLR0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }
}
