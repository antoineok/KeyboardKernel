use core::cell::UnsafeCell;

use crate::fdt::AliasEntry;

use super::address::Address;

pub const GPIO_MAX_PIN: u32 = 53;

pub struct GpioRegisters {
    sel_0: Address,
    set_0: Address,
    clr_0: Address,
    lev_0: Address,
    pup_pdn_0: Address,
}

impl GpioRegisters {
    pub const fn new() -> Self {
        GpioRegisters {
            sel_0: Address(0),
            set_0: Address(0),
            clr_0: Address(0),
            lev_0: Address(0),
            pup_pdn_0: Address(0),
        }
    }

    pub fn init(&mut self, mmio_entry: Option<AliasEntry>) {
        let base_addr = mmio_entry
            .unwrap_or_else(|| panic!("GPIO MMIO entry not found in device tree"))
            .arm_phys;
        self.sel_0.set_address(base_addr);
        self.set_0 = self.sel_0.offset(0x1C);
        self.clr_0 = self.sel_0.offset(0x28);
        self.lev_0 = self.sel_0.offset(0x34);
        self.pup_pdn_0 = self.sel_0.offset(0xE4);
    }
}

pub struct GpioRegsStatic(UnsafeCell<GpioRegisters>);

unsafe impl Sync for GpioRegsStatic {}

pub static GPIO_REGS: GpioRegsStatic = GpioRegsStatic(UnsafeCell::new(GpioRegisters::new()));

impl GpioRegsStatic {
    pub const fn get_mut(&self) -> &mut GpioRegisters {
        unsafe { &mut *self.0.get() }
    }

    pub const fn get(&self) -> &GpioRegisters {
        unsafe { &*self.0.get() }
    }
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
    pub fn new(pin: u32) -> Self {
        assert!(pin <= GPIO_MAX_PIN);
        if GPIO_REGS.get().sel_0.0 == 0 {
            panic!("GPIO registers not initialized");
        }
        GpioPin { pin }
    }

    pub fn set_function(&self, func: GpioFunction) {
        let reg = GPIO_REGS.get().sel_0.offset(((self.pin / 10) * 4) as usize);
        let shift = (self.pin % 10) * 3;
        let mut val = reg.read();
        val &= !(0b111 << shift);
        val |= (func as u32) << shift;
        reg.write(val);
    }

    pub fn set_pull(&self, pull: GpioPull) {
        let reg = GPIO_REGS
            .get()
            .pup_pdn_0
            .offset(((self.pin / 16) * 4) as usize);
        let shift = (self.pin % 16) * 2;
        let mut val = reg.read();
        val &= !(0b11 << shift);
        val |= (pull as u32) << shift;
        reg.write(val);
    }

    pub fn configure_input(&self, pull: GpioPull) {
        self.set_function(GpioFunction::Input);
        self.set_pull(pull);
    }

    pub fn level(&self) -> bool {
        let reg = GPIO_REGS.get().lev_0.offset(((self.pin / 32) * 4) as usize);
        let mask = 1u32 << (self.pin % 32);

        reg.read() & mask != 0
    }

    pub fn is_high(&self) -> bool {
        self.level()
    }

    pub fn is_low(&self) -> bool {
        !self.level()
    }

    pub fn set_high(&self) {
        let reg = GPIO_REGS.get().set_0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }

    pub fn set_low(&self) {
        let reg = GPIO_REGS.get().clr_0.offset(((self.pin / 32) * 4) as usize);
        reg.write(1 << (self.pin % 32));
    }
}
