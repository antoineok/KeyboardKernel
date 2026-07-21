//! Minimal USB device stack built on top of the low-level DWC2 driver.
//!
//! The current implementation is polling-only and implements endpoint zero
//! plus the standard requests needed for enumeration. It intentionally exposes
//! a vendor-specific interface with no additional endpoints. HID and the
//! management interface can be layered on after this enumerates reliably.

mod descriptors;
mod device;

pub use device::{SetupPacket, UsbDevice, UsbDeviceError, USB_DEVICE};
