use crate::hid::HidState;

use super::TransportState;

/// Transport selected for keyboard and management traffic.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveTransport {
    #[default]
    None,
    Usb,
    Bluetooth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportTransition {
    pub previous: ActiveTransport,
    pub current: ActiveTransport,
}

impl TransportTransition {
    pub fn changed(self) -> bool {
        self.previous != self.current
    }

    /// The old transport must receive an all-released report before it is
    /// abandoned, where possible.
    pub fn release_previous(self) -> Option<ActiveTransport> {
        if self.changed() && !matches!(self.previous, ActiveTransport::None) {
            Some(self.previous)
        } else {
            None
        }
    }
}

/// Selects USB whenever it is configured (or suspended after configuration),
/// otherwise a ready Bluetooth connection.
pub struct TransportRouter {
    active: ActiveTransport,
}

impl TransportRouter {
    pub const fn new() -> Self {
        Self {
            active: ActiveTransport::None,
        }
    }

    pub const fn active(&self) -> ActiveTransport {
        self.active
    }

    pub const fn choose(
        usb_state: TransportState,
        bluetooth_state: TransportState,
    ) -> ActiveTransport {
        if usb_state.claims_usb_priority() {
            ActiveTransport::Usb
        } else if bluetooth_state.is_ready() {
            ActiveTransport::Bluetooth
        } else {
            ActiveTransport::None
        }
    }

    /// Updates the active transport and marks all reports dirty so the newly
    /// selected host receives the complete current state.
    ///
    /// The caller should use [`TransportTransition::release_previous`] to send
    /// release reports to the previous host before routing new reports.
    pub fn update(
        &mut self,
        usb_state: TransportState,
        bluetooth_state: TransportState,
        hid_state: &mut HidState,
    ) -> TransportTransition {
        let previous = self.active;
        let current = Self::choose(usb_state, bluetooth_state);

        if previous != current {
            self.active = current;
            hid_state.mark_all_dirty();
        }

        TransportTransition { previous, current }
    }
}

impl Default for TransportRouter {
    fn default() -> Self {
        Self::new()
    }
}
