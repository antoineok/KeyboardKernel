use crate::{
    hid::{DirtyReports, HidState},
    transport::{
        smp::{try_receive_management_from_usb, try_send_management_to_usb},
        ActiveTransport, KeyworkTransport, ReceiveStatus, SendStatus, TransportKind,
        TransportRouter, TransportState, TransportTransition,
    },
};

use super::{
    management::{ManagementBackend, ManagementService},
    platform::KernelManagementBackend,
};

/// Top-level transport-independent service state.
pub struct KeyworkServices<const MANAGEMENT_BUFFER_CAPACITY: usize> {
    hid: HidState,
    management: ManagementService<MANAGEMENT_BUFFER_CAPACITY>,
    management_request: [u8; MANAGEMENT_BUFFER_CAPACITY],
    router: TransportRouter,
}

impl<const MANAGEMENT_BUFFER_CAPACITY: usize> KeyworkServices<MANAGEMENT_BUFFER_CAPACITY> {
    pub const fn new() -> Self {
        Self {
            hid: HidState::new(),
            management: ManagementService::new(),
            management_request: [0; MANAGEMENT_BUFFER_CAPACITY],
            router: TransportRouter::new(),
        }
    }

    pub const fn hid(&self) -> &HidState {
        &self.hid
    }

    pub fn hid_mut(&mut self) -> &mut HidState {
        &mut self.hid
    }

    pub fn management(&mut self) -> &mut ManagementService<MANAGEMENT_BUFFER_CAPACITY> {
        &mut self.management
    }

    pub const fn active_transport(&self) -> ActiveTransport {
        self.router.active()
    }

    pub fn update_transport_selection(
        &mut self,
        usb_state: TransportState,
        bluetooth_state: TransportState,
    ) -> TransportTransition {
        self.router
            .update(usb_state, bluetooth_state, &mut self.hid)
    }

    /// Services HID state and one KWMP request on the selected transport.
    ///
    /// Transport polling happens first so endpoint completions can free queues
    /// before dirty reports and management responses are submitted.
    pub fn poll_selected_transport<T, B>(
        &mut self,
        transport: &mut T,
        backend: &mut B,
    ) -> Result<(), T::Error>
    where
        T: KeyworkTransport,
        B: ManagementBackend,
    {
        transport.poll()?;

        let selected = match transport.kind() {
            TransportKind::Usb => ActiveTransport::Usb,
            TransportKind::Bluetooth => ActiveTransport::Bluetooth,
        };
        if self.router.active() != selected {
            return Ok(());
        }

        self.hid.set_host_leds(transport.host_led_state());
        self.flush_hid(transport)?;
        self.service_management(transport, backend)?;
        Ok(())
    }

    fn flush_hid<T: KeyworkTransport>(&mut self, transport: &mut T) -> Result<(), T::Error> {
        let dirty = self.hid.dirty();

        if dirty.contains(DirtyReports::KEYBOARD) {
            let report = self.hid.keyboard();
            if transport.send_keyboard(report)? == SendStatus::Sent {
                self.hid.acknowledge_keyboard(report);
            }
        }

        if dirty.contains(DirtyReports::CONSUMER) {
            let report = self.hid.consumer();
            if transport.send_consumer(report)? == SendStatus::Sent {
                self.hid.acknowledge_consumer(report);
            }
        }

        if dirty.contains(DirtyReports::SYSTEM) {
            let report = self.hid.system();
            if transport.send_system(report)? == SendStatus::Sent {
                self.hid.acknowledge_system(report);
            }
        }

        Ok(())
    }

    fn service_management<T: KeyworkTransport, B: ManagementBackend>(
        &mut self,
        transport: &mut T,
        backend: &mut B,
    ) -> Result<(), T::Error> {
        let length = match transport.receive_management(&mut self.management_request)? {
            ReceiveStatus::Received(length) => length,
            ReceiveStatus::Empty
            | ReceiveStatus::Disconnected
            | ReceiveStatus::BufferTooSmall { .. } => return Ok(()),
        };

        match self
            .management
            .handle_frame(backend, &self.management_request[..length])
        {
            Ok(response) => {
                let status = transport.send_management(response)?;
                debug_assert!(status != SendStatus::Busy);
            }
            Err(error) => {
                crate::warn!("Rejected KWMP frame: {:?}", error);
            }
        }
        Ok(())
    }
}

impl<const MANAGEMENT_BUFFER_CAPACITY: usize> Default
    for KeyworkServices<MANAGEMENT_BUFFER_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

/// CPU0's transport-neutral control plane.
///
/// CPU0 does not poll DWC2 or service HID endpoints. CPU2 owns USB and forwards
/// complete KWMP frames through the SMP mailbox. CPU0 only dispatches those
/// management requests to the kernel backend and queues the encoded response
/// back to the originating USB worker.
pub struct Cpu0ControlPlane<const CAPACITY: usize> {
    management: ManagementService<CAPACITY>,
    request: [u8; CAPACITY],
    pending_response: Option<BufferedManagementFrame<CAPACITY>>,
}

struct BufferedManagementFrame<const CAPACITY: usize> {
    length: usize,
    bytes: [u8; CAPACITY],
}

impl<const CAPACITY: usize> Cpu0ControlPlane<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            management: ManagementService::new(),
            request: [0; CAPACITY],
            pending_response: None,
        }
    }

    /// Service at most one queued KWMP request/response exchange.
    ///
    /// All actual USB progress happens concurrently in CPU2's permanent worker.
    /// This function only moves complete management frames across the inter-core
    /// mailbox and executes CPU0-owned control operations.
    pub fn service(&mut self, backend: &mut KernelManagementBackend) {
        if !self.flush_pending_response() {
            return;
        }

        let length = match try_receive_management_from_usb(&mut self.request) {
            ReceiveStatus::Received(length) => length,
            ReceiveStatus::Empty | ReceiveStatus::Disconnected => return,
            ReceiveStatus::BufferTooSmall { required } => {
                panic!(
                    "CPU2 supplied oversized KWMP frame: required={required}, capacity={CAPACITY}"
                )
            }
        };

        // A frame arriving through this mailbox necessarily originated from the
        // USB worker. Bluetooth will get its own ingress mailbox later.
        backend.set_active_transport(ActiveTransport::Usb);

        match self
            .management
            .handle_frame(backend, &self.request[..length])
        {
            Ok(response) => {
                let mut buffered = BufferedManagementFrame {
                    length: response.len(),
                    bytes: [0; CAPACITY],
                };
                buffered.bytes[..response.len()].copy_from_slice(response);
                self.pending_response = Some(buffered);
                let _ = self.flush_pending_response();
            }
            Err(error) => crate::warn!("Rejected KWMP frame: {:?}", error),
        }
    }

    fn flush_pending_response(&mut self) -> bool {
        let Some(response) = self.pending_response.as_ref() else {
            return true;
        };

        match try_send_management_to_usb(&response.bytes[..response.length]) {
            SendStatus::Sent => {
                self.pending_response = None;
                true
            }
            SendStatus::Busy => false,
            SendStatus::Disconnected => {
                self.pending_response = None;
                true
            }
        }
    }
}

impl<const CAPACITY: usize> Default for Cpu0ControlPlane<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
