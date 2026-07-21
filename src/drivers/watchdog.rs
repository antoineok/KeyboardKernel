use crate::early_timeit;

use super::address::Address;
use crate::fdt::AliasEntry;
use core::cell::UnsafeCell;

const PM_PASSWORD: u32 = 0x5a00_0000;
const PM_RSTC_OFFSET: usize = 0x1c;
const PM_WDOG_OFFSET: usize = 0x24;
const PM_RSTC_WRCFG_MASK: u32 = 0x0000_0030;
const PM_RSTC_WRCFG_FULL_RESET: u32 = 0x0000_0020;

pub struct Watchdog {
    initialized: bool,
    base: Address,
    rstc: Address,
    wdog: Address,
}

pub struct WatchdogStatic(UnsafeCell<Watchdog>);

unsafe impl Sync for WatchdogStatic {}

pub static WATCHDOG: WatchdogStatic = WatchdogStatic(UnsafeCell::new(Watchdog::new()));

impl WatchdogStatic {
    pub fn get_mut(&self) -> &mut Watchdog {
        unsafe { &mut *self.0.get() }
    }

    pub fn get(&self) -> &Watchdog {
        unsafe { &*self.0.get() }
    }
}

impl Watchdog {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            base: Address(0),
            rstc: Address(0),
            wdog: Address(0),
        }
    }

    pub fn init(&mut self, mmio_entry: Option<AliasEntry>) {
        let base_addr = mmio_entry
            .expect("Watchdog/PM MMIO entry not found in device tree")
            .arm_phys;

        early_timeit!("Watchdog set base", {
            self.base.set_address(base_addr);
        });

        early_timeit!("Watchdog RSTC offset", {
            self.rstc = self.base.offset(PM_RSTC_OFFSET);
        });

        early_timeit!("Watchdog WDOG offset", {
            self.wdog = self.base.offset(PM_WDOG_OFFSET);
        });

        early_timeit!("Watchdog initialized flag", {
            self.initialized = true;
        });
    }

    pub fn reboot(&self) -> ! {
        if !self.initialized {
            panic!("Watchdog not initialized");
        }

        // watchdog timeout in ticks. Small non-zero value.
        self.wdog.write(PM_PASSWORD | 10);

        let rstc = self.rstc.read();

        self.rstc.write(
            PM_PASSWORD
                | (rstc & !PM_RSTC_WRCFG_MASK)
                | PM_RSTC_WRCFG_FULL_RESET,
        );

        loop {
            core::hint::spin_loop();
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}