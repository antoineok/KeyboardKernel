//! Low-level Synopsys DWC2 USB controller driver.
//!
//! This module only owns the controller hardware. USB descriptors, endpoint
//! allocation policy, HID, composite-device handling and the management
//! protocol belong in a higher-level `crate::usb` module.

mod dwc2;
pub mod registers;

use core::cell::UnsafeCell;

pub use dwc2::{Dwc2, Dwc2Error, Dwc2Hardware, UsbSpeed};

/// Global DWC2 controller instance.
///
/// The kernel currently initializes hardware on one core during early boot.
/// Once USB interrupts or multiple cores can access the controller, ownership
/// must be tightened further (for example, by giving the interrupt handler
/// exclusive ownership or by using an interrupt-aware lock).
pub struct UsbStatic(UnsafeCell<Dwc2>);

unsafe impl Sync for UsbStatic {}

impl UsbStatic {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(Dwc2::new()))
    }

    /// Returns exclusive access to the global controller.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that no other reference to the controller
    /// exists for the returned lifetime. During early boot this means:
    ///
    /// - only one CPU is running this code;
    /// - USB interrupts are still disabled;
    /// - no other subsystem is using `USB`.
    #[inline(always)]
    pub unsafe fn get_mut(&self) -> &mut Dwc2 {
        unsafe { &mut *self.0.get() }
    }
}

pub static USB: UsbStatic = UsbStatic::new();
