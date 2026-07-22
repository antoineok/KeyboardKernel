use crate::{
    hid::{DirtyReports, HidState},
    transport::{
        ActiveTransport, KeyworkTransport, ReceiveStatus, SendStatus, TransportKind,
        TransportRouter, TransportState, TransportTransition,
    },
};

use super::management::{ManagementBackend, ManagementService};

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
