//! Keywork USB composite-device stack.

mod descriptors;
mod device;
mod transport;

pub use device::{
    SetupPacket, UsbDevice, UsbDeviceError, UsbReceiveStatus, UsbSendStatus,
    MANAGEMENT_FRAME_CAPACITY, USB_DEVICE,
};
pub use transport::{UsbTransport, USB_TRANSPORT};
