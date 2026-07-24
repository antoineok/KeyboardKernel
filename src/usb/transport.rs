use core::cell::UnsafeCell;

use crate::{
    drivers::usb::USB,
    hid::{ConsumerReport, HostLedState, KeyboardReport, SystemReport},
    transport::{KeyworkTransport, ReceiveStatus, SendStatus, TransportKind, TransportState},
};

use super::{
    UsbDeviceError, UsbReceiveStatus, UsbSendStatus, MANAGEMENT_FRAME_CAPACITY, USB_DEVICE,
};

/// USB binding for the transport-independent Keywork services.
///
/// The low-level DWC2 instance remains owned by `drivers::usb::USB`. After
/// early boot, CPU2 is the sole runtime owner of this adapter and controller.
pub struct UsbTransport {
    initialized: bool,
}

impl UsbTransport {
    pub const fn new() -> Self {
        Self { initialized: false }
    }

    pub fn initialize(&mut self) -> Result<(), UsbDeviceError> {
        let controller = unsafe { USB.get_mut() };
        let device = unsafe { USB_DEVICE.get_mut() };
        device.initialize(controller)?;
        self.initialized = true;
        Ok(())
    }

    fn device(&self) -> &super::UsbDevice {
        // CPU2 is the sole runtime owner after SMP handoff. This shared access
        // is used only for state queries between mutable operations on CPU2.
        unsafe { USB_DEVICE.get() }
    }

    fn with_device<R>(
        &mut self,
        operation: impl FnOnce(
            &mut super::UsbDevice,
            &mut crate::drivers::usb::Dwc2,
        ) -> Result<R, UsbDeviceError>,
    ) -> Result<R, UsbDeviceError> {
        let controller = unsafe { USB.get_mut() };
        let device = unsafe { USB_DEVICE.get_mut() };
        operation(device, controller)
    }
}

impl Default for UsbTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyworkTransport for UsbTransport {
    type Error = UsbDeviceError;

    fn kind(&self) -> TransportKind {
        TransportKind::Usb
    }

    fn state(&self) -> TransportState {
        if !self.initialized || !self.device().is_configured() {
            TransportState::Disconnected
        } else if self.device().is_suspended() {
            TransportState::Suspended
        } else {
            TransportState::Ready
        }
    }

    fn maximum_management_payload(&self) -> usize {
        MANAGEMENT_FRAME_CAPACITY.saturating_sub(16)
    }

    fn poll(&mut self) -> Result<(), Self::Error> {
        if !self.initialized {
            return Ok(());
        }
        self.with_device(|device, controller| device.poll(controller))
    }

    fn send_keyboard(&mut self, report: KeyboardReport) -> Result<SendStatus, Self::Error> {
        self.with_device(|device, controller| {
            device
                .send_keyboard(controller, report)
                .map(map_send_status)
        })
    }

    fn send_consumer(&mut self, report: ConsumerReport) -> Result<SendStatus, Self::Error> {
        self.with_device(|device, controller| {
            device
                .send_consumer(controller, report)
                .map(map_send_status)
        })
    }

    fn send_system(&mut self, report: SystemReport) -> Result<SendStatus, Self::Error> {
        self.with_device(|device, controller| {
            device.send_system(controller, report).map(map_send_status)
        })
    }

    fn host_led_state(&self) -> HostLedState {
        if self.initialized {
            self.device().host_led_state()
        } else {
            HostLedState::default()
        }
    }

    fn send_management(&mut self, frame: &[u8]) -> Result<SendStatus, Self::Error> {
        self.with_device(|device, controller| {
            device
                .send_management(controller, frame)
                .map(map_send_status)
        })
    }

    fn receive_management(&mut self, buffer: &mut [u8]) -> Result<ReceiveStatus, Self::Error> {
        self.with_device(|device, controller| {
            device
                .receive_management(controller, buffer)
                .map(map_receive_status)
        })
    }
}

fn map_send_status(status: UsbSendStatus) -> SendStatus {
    match status {
        UsbSendStatus::Sent => SendStatus::Sent,
        UsbSendStatus::Busy => SendStatus::Busy,
        UsbSendStatus::Disconnected => SendStatus::Disconnected,
    }
}

fn map_receive_status(status: UsbReceiveStatus) -> ReceiveStatus {
    match status {
        UsbReceiveStatus::Empty => ReceiveStatus::Empty,
        UsbReceiveStatus::Received(length) => ReceiveStatus::Received(length),
        UsbReceiveStatus::BufferTooSmall { required } => ReceiveStatus::BufferTooSmall { required },
        UsbReceiveStatus::Disconnected => ReceiveStatus::Disconnected,
    }
}

pub struct UsbTransportStatic(UnsafeCell<UsbTransport>);
unsafe impl Sync for UsbTransportStatic {}

impl UsbTransportStatic {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(UsbTransport::new()))
    }

    #[inline(always)]
    pub unsafe fn get_mut(&self) -> &mut UsbTransport {
        unsafe { &mut *self.0.get() }
    }
}

pub static USB_TRANSPORT: UsbTransportStatic = UsbTransportStatic::new();
