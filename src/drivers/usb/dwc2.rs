use core::{
    hint::spin_loop,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{compiler_fence, Ordering},
    time::Duration,
};

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use crate::{arch::time::uptime, fdt::AliasEntry};

use super::registers::*;

/// Raw hardware capability registers discovered from the DWC2 core.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Dwc2Hardware {
    pub snpsid: u32,
    pub hwcfg1: u32,
    pub hwcfg2: u32,
    pub hwcfg3: u32,
    pub hwcfg4: u32,
}

impl Dwc2Hardware {
    #[inline(always)]
    pub const fn revision(self) -> u16 {
        self.snpsid as u16
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dwc2Error {
    MissingAlias,
    RegisterWindowTooSmall {
        actual: u64,
        required: u64,
    },
    InvalidCoreId(u32),
    NotInitialized,
    Timeout {
        register: usize,
        mask: u32,
        expected_set: bool,
    },
}

/// Low-level Synopsys DWC2 controller.
///
/// This type deliberately contains no USB descriptors or class-specific state.
/// It only controls registers, FIFOs, endpoint hardware and interrupt state.
pub struct Dwc2 {
    base: u64,
    register_window_size: u64,
    hardware: Dwc2Hardware,
    initialized: bool,
}

impl Dwc2 {
    pub const fn new() -> Self {
        Self {
            base: 0,
            register_window_size: 0,
            hardware: Dwc2Hardware {
                snpsid: 0,
                hwcfg1: 0,
                hwcfg2: 0,
                hwcfg3: 0,
                hwcfg4: 0,
            },
            initialized: false,
        }
    }

    /// Maps and probes the controller, then leaves every USB interrupt masked.
    ///
    /// This is safe to call during early hardware initialization. It does not
    /// allocate memory, configure endpoints or connect the device to the host.
    pub fn init(&mut self, mmio_entry: Option<AliasEntry>) -> Result<(), Dwc2Error> {
        let entry = mmio_entry.ok_or(Dwc2Error::MissingAlias)?;

        if entry.size < MIN_REGISTER_WINDOW_SIZE {
            return Err(Dwc2Error::RegisterWindowTooSmall {
                actual: entry.size,
                required: MIN_REGISTER_WINDOW_SIZE,
            });
        }

        self.base = entry.arm_phys;
        self.register_window_size = entry.size;

        let snpsid = self.read_raw(GSNPSID);
        if snpsid & GSNPSID_OTG_MASK != GSNPSID_OTG_VALUE {
            self.base = 0;
            self.register_window_size = 0;
            return Err(Dwc2Error::InvalidCoreId(snpsid));
        }

        self.hardware = Dwc2Hardware {
            snpsid,
            hwcfg1: self.read_raw(GHWCFG1),
            hwcfg2: self.read_raw(GHWCFG2),
            hwcfg3: self.read_raw(GHWCFG3),
            hwcfg4: self.read_raw(GHWCFG4),
        };
        self.initialized = true;

        // Prevent the core from raising CPU interrupts before the kernel's USB
        // interrupt path and endpoint-zero state machine exist.
        self.disable_global_interrupts()?;
        self.write_raw(GINTMSK, 0);
        self.clear_pending_interrupts()?;

        // If the boot firmware/ID pin already selected device mode, remain
        // electrically disconnected until the higher-level USB stack is ready.
        if self.is_device_mode()? {
            self.disconnect()?;
        }

        crate::info!(
            "DWC2 initialized: base={:#x}, size={:#x}, snpsid={:#010x}",
            self.base,
            self.register_window_size,
            self.hardware.snpsid
        );
        crate::info!(
            "DWC2 capabilities: hwcfg1={:#010x}, hwcfg2={:#010x}, hwcfg3={:#010x}, hwcfg4={:#010x}",
            self.hardware.hwcfg1,
            self.hardware.hwcfg2,
            self.hardware.hwcfg3,
            self.hardware.hwcfg4
        );

        Ok(())
    }

    #[inline(always)]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    #[inline(always)]
    pub const fn base(&self) -> Option<u64> {
        if self.initialized {
            Some(self.base)
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn register_window_size(&self) -> Option<u64> {
        if self.initialized {
            Some(self.register_window_size)
        } else {
            None
        }
    }

    #[inline(always)]
    pub const fn hardware(&self) -> Option<Dwc2Hardware> {
        if self.initialized {
            Some(self.hardware)
        } else {
            None
        }
    }

    /// Prepares the controller for a future device-mode USB stack.
    ///
    /// Call this later, once the kernel is ready to build endpoint state. The
    /// controller remains soft-disconnected when this method returns.
    pub fn prepare_device_mode(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.disable_global_interrupts()?;
        self.write_raw(GINTMSK, 0);

        // Disconnect first when the controller is already in peripheral mode.
        if self.is_device_mode()? {
            self.disconnect()?;
        }

        self.core_soft_reset()?;
        self.force_device_mode()?;
        self.disconnect()?;
        self.clear_pending_interrupts()?;

        Ok(())
    }

    /// Performs a DWC2 core soft reset.
    pub fn core_soft_reset(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_AHBIDLE,
            true,
            Duration::from_millis(100),
        )?;

        self.modify_raw(GRSTCTL, |value| value | GRSTCTL_CSFTRST);

        self.wait_for_bits(
            GRSTCTL,
            GRSTCTL_CSFTRST,
            false,
            Duration::from_millis(100),
        )?;

        // The core requires a few PHY clocks after CSFTRST deasserts before
        // accessing additional registers. Three microseconds is deliberately
        // conservative for a high/full-speed PHY clock.
        self.delay(Duration::from_micros(3));

        Ok(())
    }

    /// Forces the OTG controller into peripheral/device mode.
    pub fn force_device_mode(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;

        self.modify_raw(GUSBCFG, |mut value| {
            value &= !GUSBCFG_FORCEHOSTMODE;
            value |= GUSBCFG_FORCEDEVMODE;
            value
        });

        // Mode forcing is not instantaneous. Wait for the current-mode bit,
        // then retain the standard DWC2 settling interval.
        self.wait_for_bits(
            GINTSTS,
            GINT_CURMODE_HOST,
            false,
            Duration::from_millis(100),
        )?;
        self.delay(Duration::from_millis(25));

        Ok(())
    }

    #[inline]
    pub fn is_device_mode(&self) -> Result<bool, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTSTS) & GINT_CURMODE_HOST == 0)
    }

    /// Enables the device pull-up by clearing DCTL.SFTDISCON.
    ///
    /// Do not call this until endpoint zero and the descriptor state machine
    /// are completely ready to answer the host.
    pub fn connect(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(DCTL, |value| value & !DCTL_SFTDISCON);
        self.device_barrier();
        Ok(())
    }

    /// Disables the device pull-up by setting DCTL.SFTDISCON.
    pub fn disconnect(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(DCTL, |value| value | DCTL_SFTDISCON);
        self.device_barrier();
        Ok(())
    }

    pub fn enable_global_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(GAHBCFG, |value| value | GAHBCFG_GLBL_INTR_EN);
        self.device_barrier();
        Ok(())
    }

    pub fn disable_global_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.modify_raw(GAHBCFG, |value| value & !GAHBCFG_GLBL_INTR_EN);
        self.device_barrier();
        Ok(())
    }

    pub fn set_interrupt_mask(&mut self, mask: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(GINTMSK, mask);
        self.device_barrier();
        Ok(())
    }

    pub fn interrupt_mask(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTMSK))
    }

    pub fn interrupt_status(&self) -> Result<u32, Dwc2Error> {
        self.ensure_initialized()?;
        Ok(self.read_raw(GINTSTS))
    }

    /// Acknowledges write-one-to-clear global interrupt bits.
    pub fn acknowledge_interrupts(&mut self, bits: u32) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        self.write_raw(GINTSTS, bits);
        self.device_barrier();
        Ok(())
    }

    pub fn clear_pending_interrupts(&mut self) -> Result<(), Dwc2Error> {
        self.ensure_initialized()?;
        let pending = self.read_raw(GINTSTS);

        // Bit zero reports the current host/device mode and is read-only.
        self.write_raw(GINTSTS, pending & !GINT_CURMODE_HOST);
        self.device_barrier();

        Ok(())
    }

    #[inline(always)]
    fn ensure_initialized(&self) -> Result<(), Dwc2Error> {
        if self.initialized {
            Ok(())
        } else {
            Err(Dwc2Error::NotInitialized)
        }
    }

    fn wait_for_bits(
        &self,
        register: usize,
        mask: u32,
        expected_set: bool,
        timeout: Duration,
    ) -> Result<(), Dwc2Error> {
        let started = uptime();

        loop {
            let value = self.read_raw(register);
            let condition = value & mask == mask;

            if condition == expected_set {
                return Ok(());
            }

            if uptime().saturating_sub(started) >= timeout {
                return Err(Dwc2Error::Timeout {
                    register,
                    mask,
                    expected_set,
                });
            }

            spin_loop();
        }
    }

    fn delay(&self, duration: Duration) {
        let started = uptime();
        while uptime().saturating_sub(started) < duration {
            spin_loop();
        }
    }

    #[inline(always)]
    fn modify_raw(&mut self, offset: usize, update: impl FnOnce(u32) -> u32) {
        let value = self.read_raw(offset);
        self.write_raw(offset, update(value));
    }

    #[inline(always)]
    fn read_raw(&self, offset: usize) -> u32 {
        debug_assert!(offset + core::mem::size_of::<u32>() <= self.register_window_size as usize);

        let address = (self.base as usize).wrapping_add(offset);
        unsafe { read_volatile(address as *const u32) }
    }

    #[inline(always)]
    fn write_raw(&mut self, offset: usize, value: u32) {
        debug_assert!(offset + core::mem::size_of::<u32>() <= self.register_window_size as usize);

        let address = (self.base as usize).wrapping_add(offset);
        unsafe {
            write_volatile(address as *mut u32, value);
        }
    }

    #[inline(always)]
    fn device_barrier(&self) {
        compiler_fence(Ordering::SeqCst);

        #[cfg(target_arch = "aarch64")]
        unsafe {
            asm!("dsb sy", options(nostack, preserves_flags));
        }

        compiler_fence(Ordering::SeqCst);
    }
}
