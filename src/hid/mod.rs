//! Transport-independent HID reports and state.
//!
//! USB HID and Bluetooth HID consume the same logical reports. Transport
//! modules are responsible for wrapping these reports in their own endpoint or
//! characteristic protocol.

mod keys;
mod reports;
mod state;

pub use keys::Key;
pub use reports::{
    ConsumerReport, HostLedState, KeyboardModifiers, KeyboardReport, SystemReport,
};
pub use state::{DirtyReports, HidState};
