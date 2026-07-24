//! Shared transport contracts for USB and Bluetooth.

mod router;
mod smp;

use crate::hid::{ConsumerReport, HostLedState, KeyboardReport, SystemReport};

pub use router::{ActiveTransport, TransportRouter, TransportTransition};
pub use smp::{run_usb_transport_core, usb_core_ready, UsbCoreProxy};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportKind {
    Usb,
    Bluetooth,
}

/// Connection state exposed to the routing policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportState {
    Disconnected,
    Connecting,
    Ready,
    /// USB remains the selected host while suspended, avoiding an unexpected
    /// switch to a paired Bluetooth computer.
    Suspended,
}

impl TransportState {
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    pub const fn claims_usb_priority(self) -> bool {
        matches!(self, Self::Ready | Self::Suspended)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendStatus {
    Sent,
    Busy,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveStatus {
    Empty,
    Received(usize),
    BufferTooSmall { required: usize },
    Disconnected,
}

/// Transport-facing contract shared by USB and Bluetooth implementations.
///
/// The reports and management frames are transport-neutral. USB maps them to
/// HID interrupt/bulk endpoints; Bluetooth maps them to HID-over-GATT and a
/// custom management GATT service.
pub trait KeyworkTransport {
    type Error;

    fn kind(&self) -> TransportKind;

    fn state(&self) -> TransportState;

    fn maximum_management_payload(&self) -> usize;

    fn poll(&mut self) -> Result<(), Self::Error>;

    fn send_keyboard(&mut self, report: KeyboardReport) -> Result<SendStatus, Self::Error>;

    fn send_consumer(&mut self, report: ConsumerReport) -> Result<SendStatus, Self::Error>;

    fn send_system(&mut self, report: SystemReport) -> Result<SendStatus, Self::Error>;

    fn host_led_state(&self) -> HostLedState;

    fn send_management(&mut self, frame: &[u8]) -> Result<SendStatus, Self::Error>;

    fn receive_management(&mut self, buffer: &mut [u8]) -> Result<ReceiveStatus, Self::Error>;
}
