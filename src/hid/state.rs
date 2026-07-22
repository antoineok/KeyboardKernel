use super::{ConsumerReport, HostLedState, KeyboardReport, SystemReport};

/// Bitset identifying reports that still need to reach the active transport.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct DirtyReports(u8);

impl DirtyReports {
    pub const KEYBOARD: Self = Self(1 << 0);
    pub const CONSUMER: Self = Self(1 << 1);
    pub const SYSTEM: Self = Self(1 << 2);
    pub const ALL: Self = Self(Self::KEYBOARD.0 | Self::CONSUMER.0 | Self::SYSTEM.0);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// Current logical HID state, independent of USB or Bluetooth.
///
/// Reports remain dirty until the selected transport confirms that it accepted
/// the exact state that was submitted. This prevents a release report from
/// being lost when an endpoint or Bluetooth notification is temporarily busy.
pub struct HidState {
    keyboard: KeyboardReport,
    consumer: ConsumerReport,
    system: SystemReport,
    host_leds: HostLedState,
    dirty: DirtyReports,
}

impl HidState {
    pub const fn new() -> Self {
        Self {
            keyboard: KeyboardReport::released(),
            consumer: ConsumerReport::released(),
            system: SystemReport::released(),
            host_leds: HostLedState {
                num_lock: false,
                caps_lock: false,
                scroll_lock: false,
                compose: false,
                kana: false,
            },
            dirty: DirtyReports::ALL,
        }
    }

    pub const fn keyboard(&self) -> KeyboardReport {
        self.keyboard
    }

    pub const fn consumer(&self) -> ConsumerReport {
        self.consumer
    }

    pub const fn system(&self) -> SystemReport {
        self.system
    }

    pub const fn host_leds(&self) -> HostLedState {
        self.host_leds
    }

    pub const fn dirty(&self) -> DirtyReports {
        self.dirty
    }

    pub fn set_keyboard(&mut self, report: KeyboardReport) {
        if self.keyboard != report {
            self.keyboard = report;
            self.dirty.insert(DirtyReports::KEYBOARD);
        }
    }

    pub fn set_consumer(&mut self, report: ConsumerReport) {
        if self.consumer != report {
            self.consumer = report;
            self.dirty.insert(DirtyReports::CONSUMER);
        }
    }

    pub fn set_system(&mut self, report: SystemReport) {
        if self.system != report {
            self.system = report;
            self.dirty.insert(DirtyReports::SYSTEM);
        }
    }

    pub fn set_host_leds(&mut self, state: HostLedState) {
        self.host_leds = state;
    }

    pub fn release_all(&mut self) {
        self.set_keyboard(KeyboardReport::released());
        self.set_consumer(ConsumerReport::released());
        self.set_system(SystemReport::released());
    }

    /// Forces the full state to be resent after a transport switch.
    pub fn mark_all_dirty(&mut self) {
        self.dirty = DirtyReports::ALL;
    }

    /// Marks a keyboard report sent only if the state has not changed since it
    /// was submitted to the transport.
    pub fn acknowledge_keyboard(&mut self, submitted: KeyboardReport) {
        if self.keyboard == submitted {
            self.dirty.remove(DirtyReports::KEYBOARD);
        }
    }

    pub fn acknowledge_consumer(&mut self, submitted: ConsumerReport) {
        if self.consumer == submitted {
            self.dirty.remove(DirtyReports::CONSUMER);
        }
    }

    pub fn acknowledge_system(&mut self, submitted: SystemReport) {
        if self.system == submitted {
            self.dirty.remove(DirtyReports::SYSTEM);
        }
    }
}

impl Default for HidState {
    fn default() -> Self {
        Self::new()
    }
}
