use core::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

use crate::{
    arch::cpu::{current_id, send_event, SpscQueue},
    hid::{ConsumerReport, KeyboardReport, SystemReport},
    usb::{MANAGEMENT_FRAME_CAPACITY, USB_TRANSPORT},
};

use super::{KeyworkTransport, ReceiveStatus, SendStatus, TransportState};

const HID_QUEUE_SLOTS: usize = 33;
const MANAGEMENT_QUEUE_SLOTS: usize = 3;

const STATE_DISCONNECTED: u8 = 0;
const STATE_CONNECTING: u8 = 1;
const STATE_READY: u8 = 2;
const STATE_SUSPENDED: u8 = 3;

#[derive(Clone, Copy)]
enum HidCommand {
    Keyboard(KeyboardReport),
    Consumer(ConsumerReport),
    System(SystemReport),
}

struct ManagementFrame {
    length: usize,
    bytes: [u8; MANAGEMENT_FRAME_CAPACITY],
}

impl ManagementFrame {
    fn from_slice(frame: &[u8]) -> Option<Self> {
        if frame.len() > MANAGEMENT_FRAME_CAPACITY {
            return None;
        }

        let mut bytes = [0; MANAGEMENT_FRAME_CAPACITY];
        bytes[..frame.len()].copy_from_slice(frame);
        Some(Self {
            length: frame.len(),
            bytes,
        })
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

static INPUT_TO_CPU2_HID: SpscQueue<HidCommand, HID_QUEUE_SLOTS> = SpscQueue::new();
static CPU0_TO_CPU2_MANAGEMENT: SpscQueue<ManagementFrame, MANAGEMENT_QUEUE_SLOTS> =
    SpscQueue::new();
static CPU2_TO_CPU0_MANAGEMENT: SpscQueue<ManagementFrame, MANAGEMENT_QUEUE_SLOTS> =
    SpscQueue::new();

static USB_CORE_READY: AtomicBool = AtomicBool::new(false);
static USB_STATE: AtomicU8 = AtomicU8::new(STATE_DISCONNECTED);
static USB_HOST_LEDS: AtomicU8 = AtomicU8::new(0);

/// CPU0 control-plane bridge for a transport-neutral KWMP response.
///
/// This does not poll or touch DWC2. CPU2 is the sole USB owner; this function
/// only publishes a completed management frame to CPU2's outbound mailbox.
pub(crate) fn try_send_management_to_usb(frame: &[u8]) -> SendStatus {
    if !decode_state(USB_STATE.load(Ordering::Acquire)).claims_usb_priority() {
        return SendStatus::Disconnected;
    }

    let Some(frame) = ManagementFrame::from_slice(frame) else {
        return SendStatus::Busy;
    };

    match CPU0_TO_CPU2_MANAGEMENT.try_push(frame) {
        Ok(()) => {
            send_event();
            SendStatus::Sent
        }
        Err(_) => SendStatus::Busy,
    }
}

/// CPU0 control-plane bridge for a transport-neutral KWMP request received by
/// CPU2. No USB controller work is performed here.
pub(crate) fn try_receive_management_from_usb(output: &mut [u8]) -> ReceiveStatus {
    let Some(required) = CPU2_TO_CPU0_MANAGEMENT.with_front(|frame| frame.length) else {
        return if decode_state(USB_STATE.load(Ordering::Acquire)).claims_usb_priority() {
            ReceiveStatus::Empty
        } else {
            ReceiveStatus::Disconnected
        };
    };

    if output.len() < required {
        return ReceiveStatus::BufferTooSmall { required };
    }

    let frame = CPU2_TO_CPU0_MANAGEMENT
        .try_pop()
        .expect("management queue front disappeared on its sole consumer");
    output[..frame.length].copy_from_slice(frame.as_slice());
    ReceiveStatus::Received(frame.length)
}

fn queue_hid(command: HidCommand) -> SendStatus {
    if !decode_state(USB_STATE.load(Ordering::Acquire)).claims_usb_priority() {
        return SendStatus::Disconnected;
    }

    match INPUT_TO_CPU2_HID.try_push(command) {
        Ok(()) => {
            send_event();
            SendStatus::Sent
        }
        Err(_) => SendStatus::Busy,
    }
}

/// Queue transport-neutral HID reports for CPU2. These are intentionally not
/// tied to CPU0; the future CPU3 key scanner can publish directly here.
pub(crate) fn queue_keyboard_report(report: KeyboardReport) -> SendStatus {
    queue_hid(HidCommand::Keyboard(report))
}

pub(crate) fn queue_consumer_report(report: ConsumerReport) -> SendStatus {
    queue_hid(HidCommand::Consumer(report))
}

pub(crate) fn queue_system_report(report: SystemReport) -> SendStatus {
    queue_hid(HidCommand::System(report))
}

pub fn usb_core_ready() -> bool {
    USB_CORE_READY.load(Ordering::Acquire)
}

/// Permanent CPU2 transport worker.
///
/// CPU0 performs the early power-domain/DWC2 probe before releasing CPU2.
/// From this function onward CPU2 is the sole runtime owner of
/// `USB_TRANSPORT`, `USB_DEVICE`, and the DWC2 controller singleton.
pub fn run_usb_transport_core() -> ! {
    let cpu = current_id();
    assert_eq!(cpu.raw(), 2, "USB transport worker must run on CPU2");

    let usb = unsafe { USB_TRANSPORT.get_mut() };
    usb.initialize()
        .expect("USB transport initialization failed on CPU2");

    publish_usb_snapshot(usb);
    USB_CORE_READY.store(true, Ordering::Release);
    send_event();
    crate::info!("{} USB transport worker ready", cpu);

    let mut pending_hid: Option<HidCommand> = None;
    let mut pending_management: Option<ManagementFrame> = None;
    let mut management_rx = [0u8; MANAGEMENT_FRAME_CAPACITY];

    loop {
        usb.poll().expect("CPU2 USB poll failed");
        publish_usb_snapshot(usb);

        service_hid_queue(usb, &mut pending_hid);
        service_management_tx(usb, &mut pending_management);
        service_management_rx(usb, &mut management_rx);

        // USB is still polling-driven, so CPU2 must not WFE/WFI here yet.
        spin_loop();
    }
}

fn service_hid_queue(usb: &mut crate::usb::UsbTransport, pending: &mut Option<HidCommand>) {
    if pending.is_none() {
        *pending = INPUT_TO_CPU2_HID.try_pop();
    }

    let Some(command) = *pending else {
        return;
    };

    let status = match command {
        HidCommand::Keyboard(report) => usb.send_keyboard(report),
        HidCommand::Consumer(report) => usb.send_consumer(report),
        HidCommand::System(report) => usb.send_system(report),
    }
    .expect("CPU2 USB HID send failed");

    if matches!(status, SendStatus::Sent | SendStatus::Disconnected) {
        *pending = None;
    }
}

fn service_management_tx(
    usb: &mut crate::usb::UsbTransport,
    pending: &mut Option<ManagementFrame>,
) {
    if pending.is_none() {
        *pending = CPU0_TO_CPU2_MANAGEMENT.try_pop();
    }

    let Some(frame) = pending.as_ref() else {
        return;
    };

    let status = usb
        .send_management(frame.as_slice())
        .expect("CPU2 USB management send failed");

    if matches!(status, SendStatus::Sent | SendStatus::Disconnected) {
        *pending = None;
    }
}

fn service_management_rx(
    usb: &mut crate::usb::UsbTransport,
    buffer: &mut [u8; MANAGEMENT_FRAME_CAPACITY],
) {
    // Do not consume the USB device's completed frame unless CPU0 has room to
    // receive it. `UsbDevice::receive_management` keeps the frame pending until
    // called, so this gives us backpressure without dropping KWMP requests.
    if CPU2_TO_CPU0_MANAGEMENT.is_full() {
        return;
    }

    let length = match usb
        .receive_management(buffer)
        .expect("CPU2 USB management receive failed")
    {
        ReceiveStatus::Received(length) => length,
        ReceiveStatus::Empty
        | ReceiveStatus::Disconnected
        | ReceiveStatus::BufferTooSmall { .. } => return,
    };

    let frame = ManagementFrame::from_slice(&buffer[..length])
        .expect("USB returned an oversized management frame");

    if CPU2_TO_CPU0_MANAGEMENT.try_push(frame).is_err() {
        panic!("CPU2 management RX queue became full despite single-producer reservation");
    }
    send_event();
}

fn publish_usb_snapshot(usb: &crate::usb::UsbTransport) {
    USB_HOST_LEDS.store(usb.host_led_state().bitmap(), Ordering::Release);

    let current = encode_state(usb.state());
    let previous = USB_STATE.swap(current, Ordering::AcqRel);
    if previous != current {
        crate::info!(
            "CPU2 USB transport state changed: {:?} -> {:?}",
            decode_state(previous),
            decode_state(current),
        );
    }
}

const fn encode_state(state: TransportState) -> u8 {
    match state {
        TransportState::Disconnected => STATE_DISCONNECTED,
        TransportState::Connecting => STATE_CONNECTING,
        TransportState::Ready => STATE_READY,
        TransportState::Suspended => STATE_SUSPENDED,
    }
}

const fn decode_state(state: u8) -> TransportState {
    match state {
        STATE_CONNECTING => TransportState::Connecting,
        STATE_READY => TransportState::Ready,
        STATE_SUSPENDED => TransportState::Suspended,
        _ => TransportState::Disconnected,
    }
}
