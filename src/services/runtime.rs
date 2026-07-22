use crate::{
    hid::HidState,
    transport::{ActiveTransport, TransportRouter, TransportState, TransportTransition},
};

use super::management::ManagementService;

/// Top-level transport-independent service state.
///
/// USB and Bluetooth adapters will borrow this object from the kernel loop.
/// Neither the keyboard action engine nor the management dispatcher needs to
/// know which transport is currently active.
pub struct KeyworkServices<const MANAGEMENT_RESPONSE_CAPACITY: usize> {
    hid: HidState,
    management: ManagementService<MANAGEMENT_RESPONSE_CAPACITY>,
    router: TransportRouter,
}

impl<const MANAGEMENT_RESPONSE_CAPACITY: usize>
    KeyworkServices<MANAGEMENT_RESPONSE_CAPACITY>
{
    pub const fn new() -> Self {
        Self {
            hid: HidState::new(),
            management: ManagementService::new(),
            router: TransportRouter::new(),
        }
    }

    pub const fn hid(&self) -> &HidState {
        &self.hid
    }

    pub fn hid_mut(&mut self) -> &mut HidState {
        &mut self.hid
    }

    pub fn management(&mut self) -> &mut ManagementService<MANAGEMENT_RESPONSE_CAPACITY> {
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
}

impl<const MANAGEMENT_RESPONSE_CAPACITY: usize> Default
    for KeyworkServices<MANAGEMENT_RESPONSE_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}
