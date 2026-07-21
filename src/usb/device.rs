use core::cell::UnsafeCell;

use crate::drivers::usb::{Dwc2, Dwc2Error};

use super::descriptors::{
    CONFIGURATION_DESCRIPTOR, CONFIGURATION_DESCRIPTOR_TYPE, DEVICE_DESCRIPTOR,
    DEVICE_DESCRIPTOR_TYPE, DEVICE_QUALIFIER_DESCRIPTOR,
    DEVICE_QUALIFIER_DESCRIPTOR_TYPE, OTHER_SPEED_CONFIGURATION_DESCRIPTOR,
    OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE,
};

const REQUEST_DIRECTION_IN: u8 = 0x80;
const REQUEST_TYPE_MASK: u8 = 0x60;
const REQUEST_TYPE_STANDARD: u8 = 0x00;
const REQUEST_RECIPIENT_MASK: u8 = 0x1f;
const REQUEST_RECIPIENT_DEVICE: u8 = 0x00;
const REQUEST_RECIPIENT_INTERFACE: u8 = 0x01;

const GET_STATUS: u8 = 0x00;
const CLEAR_FEATURE: u8 = 0x01;
const SET_FEATURE: u8 = 0x03;
const SET_ADDRESS: u8 = 0x05;
const GET_DESCRIPTOR: u8 = 0x06;
const GET_CONFIGURATION: u8 = 0x08;
const SET_CONFIGURATION: u8 = 0x09;
const GET_INTERFACE: u8 = 0x0a;
const SET_INTERFACE: u8 = 0x0b;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbDeviceError {
    Controller(Dwc2Error),
}

impl From<Dwc2Error> for UsbDeviceError {
    fn from(value: Dwc2Error) -> Self {
        Self::Controller(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub const fn from_bytes(bytes: [u8; 8]) -> Self {
        Self {
            request_type: bytes[0],
            request: bytes[1],
            value: u16::from_le_bytes([bytes[2], bytes[3]]),
            index: u16::from_le_bytes([bytes[4], bytes[5]]),
            length: u16::from_le_bytes([bytes[6], bytes[7]]),
        }
    }

    #[inline(always)]
    const fn direction_in(self) -> bool {
        self.request_type & REQUEST_DIRECTION_IN != 0
    }

    #[inline(always)]
    const fn is_standard(self) -> bool {
        self.request_type & REQUEST_TYPE_MASK == REQUEST_TYPE_STANDARD
    }

    #[inline(always)]
    const fn recipient(self) -> u8 {
        self.request_type & REQUEST_RECIPIENT_MASK
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlState {
    WaitingForSetup,
    DataIn,
    StatusIn,
    StatusOut,
    Stalled,
}

pub struct UsbDevice {
    state: ControlState,
    configuration: u8,
    pending_setup: Option<SetupPacket>,
    initialized: bool,
}

impl UsbDevice {
    pub const fn new() -> Self {
        Self {
            state: ControlState::WaitingForSetup,
            configuration: 0,
            pending_setup: None,
            initialized: false,
        }
    }

    /// Initializes endpoint zero and then exposes the device pull-up.
    ///
    /// The controller must already have been powered, probed and prepared for
    /// peripheral mode. Global CPU interrupt delivery remains disabled; the
    /// device is serviced by [`poll`](Self::poll).
    pub fn initialize(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        controller.initialize_device_controller()?;
        controller.arm_ep0_setup()?;

        self.state = ControlState::WaitingForSetup;
        self.configuration = 0;
        self.pending_setup = None;
        self.initialized = true;

        controller.connect()?;
        crate::info!("USB device connected; waiting for host reset");

        Ok(())
    }

    #[inline(always)]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    #[inline(always)]
    pub const fn configuration(&self) -> u8 {
        self.configuration
    }

    /// Polls all DWC2 events needed for endpoint-zero enumeration.
    pub fn poll(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        if !self.initialized {
            return Ok(());
        }

        let mut status = controller.interrupt_status()?;

        if status & crate::drivers::usb::registers::GINT_USBRST != 0 {
            controller.acknowledge_interrupts(crate::drivers::usb::registers::GINT_USBRST)?;
            controller.handle_usb_reset()?;

            self.state = ControlState::WaitingForSetup;
            self.configuration = 0;
            self.pending_setup = None;

            crate::info!("USB bus reset");
        }

        status = controller.interrupt_status()?;
        if status & crate::drivers::usb::registers::GINT_ENUMDONE != 0 {
            controller.acknowledge_interrupts(crate::drivers::usb::registers::GINT_ENUMDONE)?;
            controller.handle_enumeration_done()?;
            crate::info!("USB enumeration speed established");
        }

        // RXFLVL is level-sensitive and is cleared by draining GRXSTSP.
        while controller.interrupt_status()?
            & crate::drivers::usb::registers::GINT_RXFLVL
            != 0
        {
            self.handle_rx_fifo(controller)?;
        }

        status = controller.interrupt_status()?;
        if status & crate::drivers::usb::registers::GINT_OEPINT != 0 {
            let endpoint_interrupts = controller.ep0_out_interrupts()?;
            controller.acknowledge_ep0_out_interrupts(endpoint_interrupts)?;

            if endpoint_interrupts & crate::drivers::usb::registers::DOEPINT_XFRC != 0
                && self.state == ControlState::StatusOut
            {
                self.state = ControlState::WaitingForSetup;
                controller.arm_ep0_setup()?;
            }
        }

        status = controller.interrupt_status()?;
        if status & crate::drivers::usb::registers::GINT_IEPINT != 0 {
            let endpoint_interrupts = controller.ep0_in_interrupts()?;
            controller.acknowledge_ep0_in_interrupts(endpoint_interrupts)?;

            if endpoint_interrupts & crate::drivers::usb::registers::DIEPINT_XFRC != 0 {
                self.handle_ep0_in_complete(controller)?;
            }
        }

        Ok(())
    }

    fn handle_rx_fifo(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        let status = controller.pop_rx_status()?;
        let endpoint = (status & crate::drivers::usb::registers::GRXSTSP_EPNUM_MASK) as usize;
        let byte_count = ((status & crate::drivers::usb::registers::GRXSTSP_BCNT_MASK)
            >> crate::drivers::usb::registers::GRXSTSP_BCNT_SHIFT)
            as usize;
        let packet_status = (status & crate::drivers::usb::registers::GRXSTSP_PKTSTS_MASK)
            >> crate::drivers::usb::registers::GRXSTSP_PKTSTS_SHIFT;

        match packet_status {
            crate::drivers::usb::registers::PKTSTS_SETUP_DATA
                if endpoint == 0 && byte_count == 8 =>
            {
                let first = controller.read_fifo_word(0)?;
                let second = controller.read_fifo_word(0)?;

                // SETUP_RX only means that the eight setup bytes have reached
                // the shared RX FIFO. Do not start the data/status phase yet:
                // the DWC2 core is still completing the setup transaction.
                // The request becomes actionable when SETUP_DONE is popped.
                self.pending_setup = Some(SetupPacket::from_bytes([
                    first as u8,
                    (first >> 8) as u8,
                    (first >> 16) as u8,
                    (first >> 24) as u8,
                    second as u8,
                    (second >> 8) as u8,
                    (second >> 16) as u8,
                    (second >> 24) as u8,
                ]));
            }

            crate::drivers::usb::registers::PKTSTS_SETUP_COMPLETE
                if endpoint == 0 =>
            {
                // SETUP_DONE completes the setup phase and makes it safe to
                // program the data/status phase.
                if let Some(setup) = self.pending_setup.take() {
                    self.handle_setup(controller, setup)?;
                } else {
                    // A SETUP_DONE without the corresponding eight bytes means
                    // software lost synchronization with the shared RX FIFO.
                    self.stall(controller)?;
                }
            }

            crate::drivers::usb::registers::PKTSTS_OUT_DATA => {
                // Endpoint zero currently expects only the zero-length OUT
                // status stage. Drain any unexpected payload to keep the RX
                // FIFO synchronized, then stall the transfer.
                controller.discard_fifo_bytes(endpoint, byte_count)?;
                if byte_count != 0 {
                    self.stall(controller)?;
                }
            }

            crate::drivers::usb::registers::PKTSTS_OUT_COMPLETE => {
                // No FIFO payload accompanies this status entry.
            }

            _ => {
                controller.discard_fifo_bytes(endpoint, byte_count)?;
            }
        }

        Ok(())
    }

    fn handle_setup(
        &mut self,
        controller: &mut Dwc2,
        setup: SetupPacket,
    ) -> Result<(), UsbDeviceError> {
        if self.state == ControlState::Stalled {
            controller.clear_ep0_stall()?;
        }
        self.state = ControlState::WaitingForSetup;

        crate::info!(
            "USB SETUP: type={:#04x} req={:#04x} value={:#06x} index={:#06x} len={}",
            setup.request_type,
            setup.request,
            setup.value,
            setup.index,
            setup.length,
        );

        if !setup.is_standard() {
            return self.stall(controller);
        }

        match setup.request {
            GET_DESCRIPTOR if setup.direction_in() => {
                let descriptor_type = (setup.value >> 8) as u8;
                let descriptor_index = setup.value as u8;

                let descriptor: &[u8] = match (descriptor_type, descriptor_index) {
                    (DEVICE_DESCRIPTOR_TYPE, 0) => &DEVICE_DESCRIPTOR,
                    (CONFIGURATION_DESCRIPTOR_TYPE, 0) => &CONFIGURATION_DESCRIPTOR,
                    (DEVICE_QUALIFIER_DESCRIPTOR_TYPE, 0) => &DEVICE_QUALIFIER_DESCRIPTOR,
                    (OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE, 0) => {
                        &OTHER_SPEED_CONFIGURATION_DESCRIPTOR
                    }
                    _ => return self.stall(controller),
                };

                let length = descriptor.len().min(setup.length as usize);
                controller.start_ep0_in(&descriptor[..length])?;
                self.state = ControlState::DataIn;
            }

            SET_ADDRESS
                if !setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_DEVICE
                    && setup.index == 0
                    && setup.length == 0
                    && setup.value <= 127 =>
            {
                // DWC2's gadget programming model writes DEVADDR before
                // starting the SET_ADDRESS status ZLP.
                controller.set_device_address(setup.value as u8)?;
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
            }

            SET_CONFIGURATION
                if !setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_DEVICE
                    && setup.index == 0
                    && setup.length == 0
                    && setup.value <= 1 =>
            {
                self.configuration = setup.value as u8;
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
                crate::info!("USB configuration selected: {}", self.configuration);
            }

            GET_CONFIGURATION
                if setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_DEVICE
                    && setup.value == 0
                    && setup.index == 0 =>
            {
                let response = [self.configuration];
                let length = response.len().min(setup.length as usize);
                controller.start_ep0_in(&response[..length])?;
                self.state = ControlState::DataIn;
            }

            GET_STATUS
                if setup.direction_in()
                    && setup.value == 0
                    && matches!(
                        setup.recipient(),
                        REQUEST_RECIPIENT_DEVICE | REQUEST_RECIPIENT_INTERFACE
                    ) =>
            {
                let response = [0u8, 0u8];
                let length = response.len().min(setup.length as usize);
                controller.start_ep0_in(&response[..length])?;
                self.state = ControlState::DataIn;
            }

            GET_INTERFACE
                if setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_INTERFACE
                    && self.configuration != 0 =>
            {
                let response = [0u8];
                let length = response.len().min(setup.length as usize);
                controller.start_ep0_in(&response[..length])?;
                self.state = ControlState::DataIn;
            }

            SET_INTERFACE
                if !setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_INTERFACE
                    && setup.value == 0
                    && setup.length == 0
                    && self.configuration != 0 =>
            {
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
            }

            // No optional features are currently supported.
            CLEAR_FEATURE | SET_FEATURE => self.stall(controller)?,

            _ => self.stall(controller)?,
        }

        Ok(())
    }

    fn handle_ep0_in_complete(
        &mut self,
        controller: &mut Dwc2,
    ) -> Result<(), UsbDeviceError> {
        match self.state {
            ControlState::DataIn => {
                controller.arm_ep0_status_out()?;
                self.state = ControlState::StatusOut;
            }

            ControlState::StatusIn => {
                controller.arm_ep0_setup()?;
                self.state = ControlState::WaitingForSetup;
            }

            _ => {}
        }

        Ok(())
    }

    fn stall(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        controller.stall_ep0()?;
        self.state = ControlState::Stalled;
        Ok(())
    }
}

pub struct UsbDeviceStatic(UnsafeCell<UsbDevice>);

unsafe impl Sync for UsbDeviceStatic {}

impl UsbDeviceStatic {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(UsbDevice::new()))
    }

    /// # Safety
    ///
    /// The caller must ensure exclusive access to the USB device state. The
    /// current polling implementation assumes one active core and no USB IRQ
    /// handler.
    #[inline(always)]
    pub unsafe fn get_mut(&self) -> &mut UsbDevice {
        unsafe { &mut *self.0.get() }
    }
}

pub static USB_DEVICE: UsbDeviceStatic = UsbDeviceStatic::new();
