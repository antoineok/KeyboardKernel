use crate::fdt::MmioEntry;

use super::address::Address;

pub const GPIO_MAX_PIN: u32 = 53;

pub struct GpioRegisters {
    sel_0: Address,
    set_0: Address,
    clr_0: Address,
    pup_pdn_0: Address,
}

impl GpioRegisters {
    pub const fn new() -> Self {
        GpioRegisters {
            sel_0: Address(0),
            set_0: Address(0),
            clr_0: Address(0),
            pup_pdn_0: Address(0),
        }
    }

    pub fn init(&mut self, mmio_entry: Option<&MmioEntry>) {
        let base_addr = match mmio_entry {
            Some(entry) => entry.arm_phys,
            None => panic!("GPIO MMIO entry not found in device tree"),
        };
        self.sel_0.set_address(base_addr);
        self.set_0 = self.sel_0.offset(0x1C);
        self.clr_0 = self.sel_0.offset(0x28);
        self.pup_pdn_0 = self.sel_0.offset(0xE4);
    }
}

static mut GPIO_REGS: GpioRegisters = GpioRegisters::new();

pub const fn regs() -> &'static mut GpioRegisters {
    unsafe { &mut GPIO_REGS }
}

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
        assert!(pin <= GPIO_MAX_PIN);
        if regs().sel_0.0 == 0 {
            panic!("GPIO registers not initialized");
        }
        GpioPin { pin }
    }

    pub fn set_function(&self, func: GpioFunction) {
        let reg = regs().sel_0.offset(((self.pin / 10) * 4) as usize);
        let shift = (self.pin % 10) * 3;
        let mut val = reg.read();
        val &= !(0b111 << shift);
        val |= (func as u32) << shift;
        reg.write(val);
    }

    pub fn set_pull(&self, pull: GpioPull) {
        let reg = regs().pup_pdn_0.offset(((self.pin / 16) * 4) as usize);
        let shift = (self.pin % 16) * 2;
        let mut val = reg.read();
        val &= !(0b11 << shift);
        val |= (pull as u32) << shift;
        reg.write(val);
    }

    pub fn set_high(&self) {
        let reg = regs().set_0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }

    pub fn set_low(&self) {
        let reg = regs().clr_0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }
}
