use core::cell::UnsafeCell;

use crate::{
    drivers::usb::{registers::*, Dwc2, Dwc2Error, UsbSpeed},
    hid::{ConsumerReport, HostLedState, KeyboardReport, SystemReport},
};

use super::descriptors::*;

const REQUEST_DIRECTION_IN: u8 = 0x80;
const REQUEST_TYPE_MASK: u8 = 0x60;
const REQUEST_TYPE_STANDARD: u8 = 0x00;
const REQUEST_TYPE_CLASS: u8 = 0x20;
const REQUEST_RECIPIENT_MASK: u8 = 0x1f;
const REQUEST_RECIPIENT_DEVICE: u8 = 0x00;
const REQUEST_RECIPIENT_INTERFACE: u8 = 0x01;
const REQUEST_RECIPIENT_ENDPOINT: u8 = 0x02;

const GET_STATUS: u8 = 0x00;
const CLEAR_FEATURE: u8 = 0x01;
const SET_FEATURE: u8 = 0x03;
const SET_ADDRESS: u8 = 0x05;
const GET_DESCRIPTOR: u8 = 0x06;
const GET_CONFIGURATION: u8 = 0x08;
const SET_CONFIGURATION: u8 = 0x09;
const GET_INTERFACE: u8 = 0x0a;
const SET_INTERFACE: u8 = 0x0b;

const HID_GET_REPORT: u8 = 0x01;
const HID_GET_IDLE: u8 = 0x02;
const HID_GET_PROTOCOL: u8 = 0x03;
const HID_SET_REPORT: u8 = 0x09;
const HID_SET_IDLE: u8 = 0x0a;
const HID_SET_PROTOCOL: u8 = 0x0b;

const HID_REPORT_TYPE_INPUT: u8 = 1;
const HID_REPORT_TYPE_OUTPUT: u8 = 2;
const HID_PROTOCOL_BOOT: u8 = 0;
const HID_PROTOCOL_REPORT: u8 = 1;

pub const MANAGEMENT_FRAME_CAPACITY: usize = 4096;
const EP0_OUT_CAPACITY: usize = 64;
const EP0_IN_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbDeviceError {
    Controller(Dwc2Error),
    ManagementFrameTooLarge(usize),
    ControlResponseTooLarge(usize),
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
    const fn request_kind(self) -> u8 {
        self.request_type & REQUEST_TYPE_MASK
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
    DataOut,
    StatusIn,
    StatusOut,
    Stalled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ControlOutRequest {
    KeyboardLeds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbSendStatus {
    Sent,
    Busy,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbReceiveStatus {
    Empty,
    Received(usize),
    BufferTooSmall { required: usize },
    Disconnected,
}

pub struct UsbDevice {
    state: ControlState,
    configuration: u8,
    pending_setup: Option<SetupPacket>,
    initialized: bool,
    speed: UsbSpeed,
    suspended: bool,

    keyboard_protocol: u8,
    keyboard_idle: u8,
    extra_idle: u8,
    host_leds: HostLedState,
    last_keyboard: KeyboardReport,
    last_consumer: ConsumerReport,
    last_system: SystemReport,

    keyboard_in_busy: bool,
    extra_hid_in_busy: bool,
    management_in_busy: bool,

    control_out_request: Option<ControlOutRequest>,
    control_in: [u8; EP0_IN_CAPACITY],
    control_in_len: usize,
    control_in_offset: usize,
    control_in_packet_len: usize,
    control_in_need_zlp: bool,
    control_out: [u8; EP0_OUT_CAPACITY],
    control_out_expected: usize,
    control_out_received: usize,

    management_rx: [u8; MANAGEMENT_FRAME_CAPACITY],
    management_rx_len: usize,
    management_rx_expected: usize,
    management_rx_ready: bool,

    management_tx: [u8; MANAGEMENT_FRAME_CAPACITY],
    management_tx_len: usize,
    management_tx_offset: usize,
    management_tx_packet_len: usize,
}

impl UsbDevice {
    pub const fn new() -> Self {
        Self {
            state: ControlState::WaitingForSetup,
            configuration: 0,
            pending_setup: None,
            initialized: false,
            speed: UsbSpeed::Full,
            suspended: false,
            keyboard_protocol: HID_PROTOCOL_REPORT,
            keyboard_idle: 0,
            extra_idle: 0,
            host_leds: HostLedState {
                num_lock: false,
                caps_lock: false,
                scroll_lock: false,
                compose: false,
                kana: false,
            },
            last_keyboard: KeyboardReport::released(),
            last_consumer: ConsumerReport::released(),
            last_system: SystemReport::released(),
            keyboard_in_busy: false,
            extra_hid_in_busy: false,
            management_in_busy: false,
            control_out_request: None,
            control_in: [0; EP0_IN_CAPACITY],
            control_in_len: 0,
            control_in_offset: 0,
            control_in_packet_len: 0,
            control_in_need_zlp: false,
            control_out: [0; EP0_OUT_CAPACITY],
            control_out_expected: 0,
            control_out_received: 0,
            management_rx: [0; MANAGEMENT_FRAME_CAPACITY],
            management_rx_len: 0,
            management_rx_expected: 0,
            management_rx_ready: false,
            management_tx: [0; MANAGEMENT_FRAME_CAPACITY],
            management_tx_len: 0,
            management_tx_offset: 0,
            management_tx_packet_len: 0,
        }
    }

    pub fn initialize(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        controller.initialize_device_controller()?;
        controller.arm_ep0_setup()?;
        self.reset_software_state();
        self.initialized = true;
        controller.connect()?;
        crate::info!("USB composite device connected; waiting for host reset");
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

    #[inline(always)]
    pub const fn is_configured(&self) -> bool {
        self.configuration != 0
    }

    #[inline(always)]
    pub const fn is_suspended(&self) -> bool {
        self.suspended
    }

    #[inline(always)]
    pub const fn host_led_state(&self) -> HostLedState {
        self.host_leds
    }

    pub fn poll(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        if !self.initialized {
            return Ok(());
        }

        let mut status = controller.interrupt_status()?;
        if status & GINT_USBRST != 0 {
            controller.acknowledge_interrupts(GINT_USBRST)?;
            controller.handle_usb_reset()?;
            self.reset_software_state();
            crate::info!("USB bus reset");
        }

        status = controller.interrupt_status()?;
        if status & GINT_ENUMDONE != 0 {
            controller.acknowledge_interrupts(GINT_ENUMDONE)?;
            controller.handle_enumeration_done()?;
            self.speed = controller.enumerated_speed()?;
            crate::info!("USB enumeration speed established: {:?}", self.speed);
        }

        status = controller.interrupt_status()?;
        if status & GINT_USBSUSP != 0 {
            controller.acknowledge_interrupts(GINT_USBSUSP)?;
            self.suspended = self.configuration != 0;
        }
        if status & GINT_WKUPINT != 0 {
            controller.acknowledge_interrupts(GINT_WKUPINT)?;
            self.suspended = false;
        }
        if status & GINT_DISCONNINT != 0 {
            controller.acknowledge_interrupts(GINT_DISCONNINT)?;
            self.configuration = 0;
            self.suspended = false;
            self.clear_endpoint_state();
        }

        while controller.interrupt_status()? & GINT_RXFLVL != 0 {
            self.handle_rx_fifo(controller)?;
        }

        if controller.interrupt_status()? & GINT_OEPINT != 0 {
            self.handle_out_endpoint_interrupts(controller)?;
        }
        if controller.interrupt_status()? & GINT_IEPINT != 0 {
            self.handle_in_endpoint_interrupts(controller)?;
        }

        Ok(())
    }

    pub fn send_keyboard(
        &mut self,
        controller: &mut Dwc2,
        report: KeyboardReport,
    ) -> Result<UsbSendStatus, UsbDeviceError> {
        if !self.is_configured() {
            return Ok(UsbSendStatus::Disconnected);
        }
        if self.keyboard_in_busy {
            return Ok(UsbSendStatus::Busy);
        }
        let bytes = report.encode();
        controller.start_in_transfer(
            KEYBOARD_ENDPOINT as usize,
            KEYBOARD_MAX_PACKET_SIZE as usize,
            &bytes,
        )?;
        self.last_keyboard = report;
        self.keyboard_in_busy = true;
        Ok(UsbSendStatus::Sent)
    }

    pub fn send_consumer(
        &mut self,
        controller: &mut Dwc2,
        report: ConsumerReport,
    ) -> Result<UsbSendStatus, UsbDeviceError> {
        if !self.is_configured() {
            return Ok(UsbSendStatus::Disconnected);
        }
        if self.extra_hid_in_busy {
            return Ok(UsbSendStatus::Busy);
        }
        let usage = report.encode();
        let bytes = [EXTRA_REPORT_ID_CONSUMER, usage[0], usage[1]];
        controller.start_in_transfer(
            EXTRA_HID_ENDPOINT as usize,
            EXTRA_HID_MAX_PACKET_SIZE as usize,
            &bytes,
        )?;
        self.last_consumer = report;
        self.extra_hid_in_busy = true;
        Ok(UsbSendStatus::Sent)
    }

    pub fn send_system(
        &mut self,
        controller: &mut Dwc2,
        report: SystemReport,
    ) -> Result<UsbSendStatus, UsbDeviceError> {
        if !self.is_configured() {
            return Ok(UsbSendStatus::Disconnected);
        }
        if self.extra_hid_in_busy {
            return Ok(UsbSendStatus::Busy);
        }
        let bytes = [EXTRA_REPORT_ID_SYSTEM, report.usage];
        controller.start_in_transfer(
            EXTRA_HID_ENDPOINT as usize,
            EXTRA_HID_MAX_PACKET_SIZE as usize,
            &bytes,
        )?;
        self.last_system = report;
        self.extra_hid_in_busy = true;
        Ok(UsbSendStatus::Sent)
    }

    pub fn send_management(
        &mut self,
        controller: &mut Dwc2,
        frame: &[u8],
    ) -> Result<UsbSendStatus, UsbDeviceError> {
        if !self.is_configured() {
            return Ok(UsbSendStatus::Disconnected);
        }
        if self.management_tx_len != 0 || self.management_in_busy {
            return Ok(UsbSendStatus::Busy);
        }
        if frame.len() > MANAGEMENT_FRAME_CAPACITY {
            return Err(UsbDeviceError::ManagementFrameTooLarge(frame.len()));
        }
        self.management_tx[..frame.len()].copy_from_slice(frame);
        self.management_tx_len = frame.len();
        self.management_tx_offset = 0;
        self.start_next_management_packet(controller)?;
        Ok(UsbSendStatus::Sent)
    }

    pub fn receive_management(
        &mut self,
        controller: &mut Dwc2,
        output: &mut [u8],
    ) -> Result<UsbReceiveStatus, UsbDeviceError> {
        if !self.is_configured() {
            return Ok(UsbReceiveStatus::Disconnected);
        }
        if !self.management_rx_ready || self.management_tx_len != 0 || self.management_in_busy {
            return Ok(UsbReceiveStatus::Empty);
        }
        if output.len() < self.management_rx_len {
            return Ok(UsbReceiveStatus::BufferTooSmall {
                required: self.management_rx_len,
            });
        }
        let length = self.management_rx_len;
        output[..length].copy_from_slice(&self.management_rx[..length]);
        self.reset_management_rx();
        controller.arm_out_endpoint(
            MANAGEMENT_ENDPOINT as usize,
            self.speed.management_packet_size(),
        )?;
        Ok(UsbReceiveStatus::Received(length))
    }

    fn handle_rx_fifo(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        let status = controller.pop_rx_status()?;
        let endpoint = (status & GRXSTSP_EPNUM_MASK) as usize;
        let byte_count = ((status & GRXSTSP_BCNT_MASK) >> GRXSTSP_BCNT_SHIFT) as usize;
        let packet_status = (status & GRXSTSP_PKTSTS_MASK) >> GRXSTSP_PKTSTS_SHIFT;

        match packet_status {
            PKTSTS_SETUP_DATA if endpoint == 0 && byte_count == 8 => {
                let first = controller.read_fifo_word(0)?;
                let second = controller.read_fifo_word(0)?;
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
            PKTSTS_SETUP_COMPLETE if endpoint == 0 => {
                if let Some(setup) = self.pending_setup.take() {
                    self.handle_setup(controller, setup)?;
                } else {
                    self.stall(controller)?;
                }
            }
            // A control-read transfer ends with a zero-length OUT status
            // packet. DWC2 reports that packet through RXFLVL before the
            // endpoint transfer-complete interrupt; it is not a control OUT
            // data stage and must not stall EP0.
            PKTSTS_OUT_DATA
                if endpoint == 0 && self.state == ControlState::StatusOut && byte_count == 0 => {}
            PKTSTS_OUT_DATA if endpoint == 0 && self.state == ControlState::DataOut => {
                self.read_control_out(controller, byte_count)?;
            }
            PKTSTS_OUT_DATA if endpoint == 0 => {
                controller.discard_fifo_bytes(0, byte_count)?;
                self.stall(controller)?;
            }
            PKTSTS_OUT_DATA if endpoint == MANAGEMENT_ENDPOINT as usize => {
                self.read_management_out(controller, byte_count)?;
            }
            PKTSTS_OUT_DATA => {
                controller.discard_fifo_bytes(endpoint, byte_count)?;
            }
            PKTSTS_OUT_COMPLETE | PKTSTS_SETUP_COMPLETE => {}
            _ => controller.discard_fifo_bytes(endpoint, byte_count)?,
        }
        Ok(())
    }

    fn handle_out_endpoint_interrupts(
        &mut self,
        controller: &mut Dwc2,
    ) -> Result<(), UsbDeviceError> {
        let summary = controller.endpoint_interrupt_summary()?;

        if summary & daint_out(0) != 0 {
            let interrupts = controller.endpoint_out_interrupts(0)?;
            controller.acknowledge_endpoint_out_interrupts(0, interrupts)?;
            if interrupts & DOEPINT_XFRC != 0 {
                match self.state {
                    ControlState::StatusOut => {
                        self.finish_control_transfer(controller)?;
                    }
                    ControlState::DataOut => self.complete_control_out(controller)?,
                    _ => {}
                }
            }
        }

        let management_endpoint = MANAGEMENT_ENDPOINT as usize;
        if summary & daint_out(management_endpoint) != 0 {
            let interrupts = controller.endpoint_out_interrupts(management_endpoint)?;
            controller.acknowledge_endpoint_out_interrupts(management_endpoint, interrupts)?;
            if interrupts & DOEPINT_XFRC != 0 && !self.management_rx_ready {
                controller
                    .arm_out_endpoint(management_endpoint, self.speed.management_packet_size())?;
            }
        }
        Ok(())
    }

    fn handle_in_endpoint_interrupts(
        &mut self,
        controller: &mut Dwc2,
    ) -> Result<(), UsbDeviceError> {
        let summary = controller.endpoint_interrupt_summary()?;
        for endpoint in 0..=3 {
            if summary & daint_in(endpoint) == 0 {
                continue;
            }
            let interrupts = controller.endpoint_in_interrupts(endpoint)?;
            controller.acknowledge_endpoint_in_interrupts(endpoint, interrupts)?;
            if interrupts & DIEPINT_XFRC == 0 {
                continue;
            }
            match endpoint {
                0 => self.handle_ep0_in_complete(controller)?,
                endpoint if endpoint == KEYBOARD_ENDPOINT as usize => {
                    self.keyboard_in_busy = false;
                }
                endpoint if endpoint == EXTRA_HID_ENDPOINT as usize => {
                    self.extra_hid_in_busy = false;
                }
                endpoint if endpoint == MANAGEMENT_ENDPOINT as usize => {
                    self.management_in_busy = false;
                    self.management_tx_offset = self
                        .management_tx_offset
                        .saturating_add(self.management_tx_packet_len);
                    self.management_tx_packet_len = 0;
                    if self.management_tx_offset < self.management_tx_len {
                        self.start_next_management_packet(controller)?;
                    } else {
                        self.management_tx_len = 0;
                        self.management_tx_offset = 0;
                    }
                }
                _ => {}
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
        self.control_out_request = None;
        self.control_in_len = 0;
        self.control_in_offset = 0;
        self.control_in_packet_len = 0;
        self.control_in_need_zlp = false;
        self.control_out_expected = 0;
        self.control_out_received = 0;

        match setup.request_kind() {
            REQUEST_TYPE_STANDARD => self.handle_standard_setup(controller, setup),
            REQUEST_TYPE_CLASS if setup.recipient() == REQUEST_RECIPIENT_INTERFACE => {
                self.handle_hid_setup(controller, setup)
            }
            _ => self.stall(controller),
        }
    }

    fn handle_standard_setup(
        &mut self,
        controller: &mut Dwc2,
        setup: SetupPacket,
    ) -> Result<(), UsbDeviceError> {
        match setup.request {
            GET_DESCRIPTOR if setup.direction_in() => {
                let descriptor_type = (setup.value >> 8) as u8;
                let descriptor_index = setup.value as u8;
                let descriptor = self.descriptor_for(setup, descriptor_type, descriptor_index);
                let descriptor = match descriptor {
                    Some(value) => value,
                    None => return self.stall(controller),
                };
                self.control_in(controller, setup, descriptor)?;
            }
            SET_ADDRESS
                if !setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_DEVICE
                    && setup.index == 0
                    && setup.length == 0
                    && setup.value <= 127 =>
            {
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
                if setup.value == 1 && self.speed == UsbSpeed::Low {
                    return self.stall(controller);
                }
                self.configuration = setup.value as u8;
                self.suspended = false;
                self.clear_endpoint_state();
                if self.configuration == 1 {
                    controller.configure_composite_endpoints(
                        KEYBOARD_ENDPOINT as usize,
                        MANAGEMENT_ENDPOINT as usize,
                        EXTRA_HID_ENDPOINT as usize,
                    )?;
                } else {
                    controller.deconfigure_non_control_endpoints()?;
                }
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
                self.control_in(controller, setup, &[self.configuration])?;
            }
            GET_STATUS
                if setup.direction_in()
                    && setup.value == 0
                    && matches!(
                        setup.recipient(),
                        REQUEST_RECIPIENT_DEVICE
                            | REQUEST_RECIPIENT_INTERFACE
                            | REQUEST_RECIPIENT_ENDPOINT
                    ) =>
            {
                self.control_in(controller, setup, &[0, 0])?;
            }
            GET_INTERFACE
                if setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_INTERFACE
                    && self.configuration != 0
                    && setup.index <= MANAGEMENT_INTERFACE as u16 =>
            {
                self.control_in(controller, setup, &[0])?;
            }
            SET_INTERFACE
                if !setup.direction_in()
                    && setup.recipient() == REQUEST_RECIPIENT_INTERFACE
                    && setup.value == 0
                    && setup.length == 0
                    && setup.index <= MANAGEMENT_INTERFACE as u16
                    && self.configuration != 0 =>
            {
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
            }
            CLEAR_FEATURE | SET_FEATURE => self.stall(controller)?,
            _ => self.stall(controller)?,
        }
        Ok(())
    }

    fn handle_hid_setup(
        &mut self,
        controller: &mut Dwc2,
        setup: SetupPacket,
    ) -> Result<(), UsbDeviceError> {
        let interface = setup.index as u8;
        if interface != KEYBOARD_INTERFACE && interface != EXTRA_HID_INTERFACE {
            return self.stall(controller);
        }

        match setup.request {
            HID_SET_IDLE if !setup.direction_in() && setup.length == 0 => {
                let duration = (setup.value >> 8) as u8;
                if interface == KEYBOARD_INTERFACE {
                    self.keyboard_idle = duration;
                } else {
                    self.extra_idle = duration;
                }
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
            }
            HID_GET_IDLE if setup.direction_in() => {
                let idle = if interface == KEYBOARD_INTERFACE {
                    self.keyboard_idle
                } else {
                    self.extra_idle
                };
                self.control_in(controller, setup, &[idle])?;
            }
            HID_SET_PROTOCOL
                if !setup.direction_in()
                    && setup.length == 0
                    && setup.value <= HID_PROTOCOL_REPORT as u16 =>
            {
                if interface == KEYBOARD_INTERFACE {
                    self.keyboard_protocol = setup.value as u8;
                } else if setup.value != HID_PROTOCOL_REPORT as u16 {
                    return self.stall(controller);
                }
                controller.start_ep0_status_in()?;
                self.state = ControlState::StatusIn;
            }
            HID_GET_PROTOCOL if setup.direction_in() => {
                let protocol = if interface == KEYBOARD_INTERFACE {
                    self.keyboard_protocol
                } else {
                    HID_PROTOCOL_REPORT
                };
                self.control_in(controller, setup, &[protocol])?;
            }
            HID_SET_REPORT
                if !setup.direction_in()
                    && interface == KEYBOARD_INTERFACE
                    && (setup.value >> 8) as u8 == HID_REPORT_TYPE_OUTPUT
                    && setup.value as u8 == 0
                    && setup.length == 1 =>
            {
                self.control_out_request = Some(ControlOutRequest::KeyboardLeds);
                self.control_out_expected = 1;
                self.control_out_received = 0;
                controller.arm_ep0_out(1)?;
                self.state = ControlState::DataOut;
            }
            HID_GET_REPORT if setup.direction_in() => {
                let report_type = (setup.value >> 8) as u8;
                let report_id = setup.value as u8;
                match (interface, report_type, report_id) {
                    (KEYBOARD_INTERFACE, HID_REPORT_TYPE_INPUT, 0) => {
                        let report = self.last_keyboard.encode();
                        self.control_in(controller, setup, &report)?;
                    }
                    (KEYBOARD_INTERFACE, HID_REPORT_TYPE_OUTPUT, 0) => {
                        self.control_in(controller, setup, &[self.host_leds.bitmap()])?;
                    }
                    (EXTRA_HID_INTERFACE, HID_REPORT_TYPE_INPUT, EXTRA_REPORT_ID_CONSUMER) => {
                        let usage = self.last_consumer.encode();
                        self.control_in(
                            controller,
                            setup,
                            &[EXTRA_REPORT_ID_CONSUMER, usage[0], usage[1]],
                        )?;
                    }
                    (EXTRA_HID_INTERFACE, HID_REPORT_TYPE_INPUT, EXTRA_REPORT_ID_SYSTEM) => {
                        self.control_in(
                            controller,
                            setup,
                            &[EXTRA_REPORT_ID_SYSTEM, self.last_system.usage],
                        )?;
                    }
                    _ => return self.stall(controller),
                }
            }
            _ => return self.stall(controller),
        }
        Ok(())
    }

    fn descriptor_for(
        &self,
        setup: SetupPacket,
        descriptor_type: u8,
        descriptor_index: u8,
    ) -> Option<&'static [u8]> {
        match (descriptor_type, descriptor_index) {
            (DEVICE_DESCRIPTOR_TYPE, 0) => Some(&DEVICE_DESCRIPTOR),
            (CONFIGURATION_DESCRIPTOR_TYPE, 0) => Some(match self.speed {
                UsbSpeed::High => &CONFIGURATION_DESCRIPTOR_HIGH_SPEED,
                UsbSpeed::Full | UsbSpeed::Low => &CONFIGURATION_DESCRIPTOR_FULL_SPEED,
            }),
            (STRING_DESCRIPTOR_TYPE, index) => string_descriptor(index, setup.index),
            (DEVICE_QUALIFIER_DESCRIPTOR_TYPE, 0) => Some(&DEVICE_QUALIFIER_DESCRIPTOR),
            (OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE, 0) => Some(match self.speed {
                UsbSpeed::High => &OTHER_SPEED_CONFIGURATION_DESCRIPTOR_FULL_SPEED,
                UsbSpeed::Full | UsbSpeed::Low => &OTHER_SPEED_CONFIGURATION_DESCRIPTOR_HIGH_SPEED,
            }),
            (HID_DESCRIPTOR_TYPE, 0) if setup.index == KEYBOARD_INTERFACE as u16 => {
                Some(&KEYBOARD_HID_DESCRIPTOR)
            }
            (HID_DESCRIPTOR_TYPE, 0) if setup.index == EXTRA_HID_INTERFACE as u16 => {
                Some(&EXTRA_HID_DESCRIPTOR)
            }
            (HID_REPORT_DESCRIPTOR_TYPE, 0) if setup.index == KEYBOARD_INTERFACE as u16 => {
                Some(&KEYBOARD_REPORT_DESCRIPTOR)
            }
            (HID_REPORT_DESCRIPTOR_TYPE, 0) if setup.index == EXTRA_HID_INTERFACE as u16 => {
                Some(&EXTRA_HID_REPORT_DESCRIPTOR)
            }
            _ => None,
        }
    }

    fn control_in(
        &mut self,
        controller: &mut Dwc2,
        setup: SetupPacket,
        response: &[u8],
    ) -> Result<(), UsbDeviceError> {
        let length = response.len().min(setup.length as usize);
        if length > EP0_IN_CAPACITY {
            return Err(UsbDeviceError::ControlResponseTooLarge(length));
        }
        self.control_in[..length].copy_from_slice(&response[..length]);
        self.control_in_len = length;
        self.control_in_offset = 0;
        self.control_in_packet_len = 0;
        let packet_size = self.ep0_packet_size();
        self.control_in_need_zlp = length < setup.length as usize && length % packet_size == 0;
        self.state = ControlState::DataIn;
        self.start_next_control_in_packet(controller)
    }

    fn start_next_control_in_packet(
        &mut self,
        controller: &mut Dwc2,
    ) -> Result<(), UsbDeviceError> {
        let remaining = self.control_in_len.saturating_sub(self.control_in_offset);
        if remaining != 0 {
            let packet_length = remaining.min(self.ep0_packet_size());
            let start = self.control_in_offset;
            controller.start_ep0_in(&self.control_in[start..start + packet_length])?;
            self.control_in_packet_len = packet_length;
            return Ok(());
        }
        if self.control_in_need_zlp {
            self.control_in_need_zlp = false;
            self.control_in_packet_len = 0;
            controller.start_ep0_status_in()?;
            return Ok(());
        }
        controller.arm_ep0_status_out()?;
        self.state = ControlState::StatusOut;
        Ok(())
    }

    const fn ep0_packet_size(&self) -> usize {
        match self.speed {
            UsbSpeed::Low => 8,
            UsbSpeed::High | UsbSpeed::Full => 64,
        }
    }

    fn read_control_out(
        &mut self,
        controller: &mut Dwc2,
        byte_count: usize,
    ) -> Result<(), UsbDeviceError> {
        if self.state != ControlState::DataOut
            || self.control_out_received.saturating_add(byte_count) > self.control_out_expected
        {
            controller.discard_fifo_bytes(0, byte_count)?;
            return self.stall(controller);
        }
        let start = self.control_out_received;
        Self::read_fifo_bytes(
            controller,
            0,
            byte_count,
            &mut self.control_out[start..start + byte_count],
        )?;
        self.control_out_received += byte_count;
        Ok(())
    }

    fn complete_control_out(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        if self.control_out_received != self.control_out_expected {
            return self.stall(controller);
        }
        match self.control_out_request.take() {
            Some(ControlOutRequest::KeyboardLeds) => {
                self.host_leds = HostLedState::from_bitmap(self.control_out[0]);
            }
            None => return self.stall(controller),
        }
        controller.start_ep0_status_in()?;
        self.state = ControlState::StatusIn;
        Ok(())
    }

    fn read_management_out(
        &mut self,
        controller: &mut Dwc2,
        byte_count: usize,
    ) -> Result<(), UsbDeviceError> {
        let attempted_length = self.management_rx_len.saturating_add(byte_count);
        if self.management_rx_ready || attempted_length > MANAGEMENT_FRAME_CAPACITY {
            controller.discard_fifo_bytes(MANAGEMENT_ENDPOINT as usize, byte_count)?;
            self.reset_management_rx();
            crate::warn!(
                "Dropped oversized KWMP transfer: {} bytes",
                attempted_length
            );
            return Ok(());
        }

        let start = self.management_rx_len;
        Self::read_fifo_bytes(
            controller,
            MANAGEMENT_ENDPOINT as usize,
            byte_count,
            &mut self.management_rx[start..start + byte_count],
        )?;
        self.management_rx_len += byte_count;

        if self.management_rx_expected == 0 && self.management_rx_len >= 16 {
            let payload_length = u32::from_le_bytes([
                self.management_rx[12],
                self.management_rx[13],
                self.management_rx[14],
                self.management_rx[15],
            ]) as usize;
            let expected = 16usize.checked_add(payload_length);
            match expected {
                Some(expected) if expected <= MANAGEMENT_FRAME_CAPACITY => {
                    self.management_rx_expected = expected;
                }
                _ => {
                    self.reset_management_rx();
                    crate::warn!("Dropped KWMP frame with invalid declared length");
                    return Ok(());
                }
            }
        }

        if self.management_rx_expected != 0 {
            if self.management_rx_len == self.management_rx_expected {
                self.management_rx_ready = true;
            } else if self.management_rx_len > self.management_rx_expected {
                self.reset_management_rx();
                crate::warn!("Dropped concatenated or malformed KWMP frame");
            }
        }
        Ok(())
    }

    fn start_next_management_packet(
        &mut self,
        controller: &mut Dwc2,
    ) -> Result<(), UsbDeviceError> {
        let remaining = self
            .management_tx_len
            .saturating_sub(self.management_tx_offset);
        if remaining == 0 {
            self.management_tx_len = 0;
            return Ok(());
        }
        let packet_length = remaining.min(self.speed.management_packet_size());
        let start = self.management_tx_offset;
        controller.start_in_transfer(
            MANAGEMENT_ENDPOINT as usize,
            self.speed.management_packet_size(),
            &self.management_tx[start..start + packet_length],
        )?;
        self.management_tx_packet_len = packet_length;
        self.management_in_busy = true;
        Ok(())
    }

    fn handle_ep0_in_complete(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        match self.state {
            ControlState::DataIn => {
                self.control_in_offset = self
                    .control_in_offset
                    .saturating_add(self.control_in_packet_len);
                self.control_in_packet_len = 0;
                self.start_next_control_in_packet(controller)?;
            }
            ControlState::StatusIn => {
                self.finish_control_transfer(controller)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_control_transfer(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        self.state = ControlState::WaitingForSetup;
        self.pending_setup = None;
        self.control_out_request = None;
        self.control_in_len = 0;
        self.control_in_offset = 0;
        self.control_in_packet_len = 0;
        self.control_in_need_zlp = false;
        self.control_out_expected = 0;
        self.control_out_received = 0;
        controller.arm_ep0_setup()?;
        Ok(())
    }

    fn stall(&mut self, controller: &mut Dwc2) -> Result<(), UsbDeviceError> {
        controller.stall_ep0()?;
        self.state = ControlState::Stalled;
        Ok(())
    }

    fn read_fifo_bytes(
        controller: &Dwc2,
        _endpoint: usize,
        byte_count: usize,
        output: &mut [u8],
    ) -> Result<(), UsbDeviceError> {
        debug_assert!(output.len() >= byte_count);
        let mut written = 0usize;
        for _ in 0..byte_count.div_ceil(4) {
            // Every OUT endpoint is drained through the shared receive FIFO.
            let word = controller.read_fifo_word(0)?;
            for byte_index in 0..4 {
                if written == byte_count {
                    break;
                }
                output[written] = (word >> (byte_index * 8)) as u8;
                written += 1;
            }
        }
        Ok(())
    }

    fn reset_software_state(&mut self) {
        self.state = ControlState::WaitingForSetup;
        self.configuration = 0;
        self.pending_setup = None;
        self.speed = UsbSpeed::Full;
        self.suspended = false;
        self.keyboard_protocol = HID_PROTOCOL_REPORT;
        self.keyboard_idle = 0;
        self.extra_idle = 0;
        self.control_out_request = None;
        self.control_in_len = 0;
        self.control_in_offset = 0;
        self.control_in_packet_len = 0;
        self.control_in_need_zlp = false;
        self.control_out_expected = 0;
        self.control_out_received = 0;
        self.clear_endpoint_state();
        self.reset_management_rx();
    }

    fn clear_endpoint_state(&mut self) {
        self.keyboard_in_busy = false;
        self.extra_hid_in_busy = false;
        self.management_in_busy = false;
        self.management_tx_len = 0;
        self.management_tx_offset = 0;
        self.management_tx_packet_len = 0;
    }

    fn reset_management_rx(&mut self) {
        self.management_rx_len = 0;
        self.management_rx_expected = 0;
        self.management_rx_ready = false;
    }
}

pub struct UsbDeviceStatic(UnsafeCell<UsbDevice>);
unsafe impl Sync for UsbDeviceStatic {}

impl UsbDeviceStatic {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(UsbDevice::new()))
    }

    #[inline(always)]
    pub unsafe fn get(&self) -> &UsbDevice {
        unsafe { &*self.0.get() }
    }

    #[inline(always)]
    pub unsafe fn get_mut(&self) -> &mut UsbDevice {
        unsafe { &mut *self.0.get() }
    }
}

pub static USB_DEVICE: UsbDeviceStatic = UsbDeviceStatic::new();
