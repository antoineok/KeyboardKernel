//! Logical HID reports shared by USB and Bluetooth transports.

use crate::hid::keys::Key;

/// Modifier bitmap used by a boot-protocol keyboard report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct KeyboardModifiers(u8);

impl KeyboardModifiers {
    pub const LEFT_CONTROL: Self = Self(1 << 0);
    pub const LEFT_SHIFT: Self = Self(1 << 1);
    pub const LEFT_ALT: Self = Self(1 << 2);
    pub const LEFT_GUI: Self = Self(1 << 3);
    pub const RIGHT_CONTROL: Self = Self(1 << 4);
    pub const RIGHT_SHIFT: Self = Self(1 << 5);
    pub const RIGHT_ALT: Self = Self(1 << 6);
    pub const RIGHT_GUI: Self = Self(1 << 7);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, modifier: Self) -> bool {
        self.0 & modifier.0 == modifier.0
    }

    pub fn insert(&mut self, modifier: Self) {
        self.0 |= modifier.0;
    }

    pub fn remove(&mut self, modifier: Self) {
        self.0 &= !modifier.0;
    }
}

impl core::ops::BitOr for KeyboardModifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for KeyboardModifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Standard eight-byte boot-keyboard input report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyboardReport {
    pub modifiers: KeyboardModifiers,
    pub keys: [Key; 6],
}

impl KeyboardReport {
    pub const BYTE_LEN: usize = 8;

    pub const fn released() -> Self {
        Self {
            modifiers: KeyboardModifiers::from_bits(0),
            keys: [Key::None; 6],
        }
    }

    /// Encodes the report without relying on structure layout or alignment.
    pub fn encode(self) -> [u8; Self::BYTE_LEN] {
        [
            self.modifiers.bits(),
            0, // Reserved byte.
            self.keys[0] as u8,
            self.keys[1] as u8,
            self.keys[2] as u8,
            self.keys[3] as u8,
            self.keys[4] as u8,
            self.keys[5] as u8,
        ]
    }
}

/// Consumer-page report, for media and volume controls.
///
/// `usage == 0` represents the released state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConsumerReport {
    pub usage: u16,
}

impl ConsumerReport {
    pub const BYTE_LEN: usize = 2;

    pub const fn released() -> Self {
        Self { usage: 0 }
    }

    pub const fn encode(self) -> [u8; Self::BYTE_LEN] {
        self.usage.to_le_bytes()
    }
}

/// System-control report, for power, sleep, and wake usages.
///
/// `usage == 0` represents the released state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemReport {
    pub usage: u8,
}

impl SystemReport {
    pub const BYTE_LEN: usize = 1;

    pub const fn released() -> Self {
        Self { usage: 0 }
    }

    pub const fn encode(self) -> [u8; Self::BYTE_LEN] {
        [self.usage]
    }
}

/// Lock-LED state supplied by the active host.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostLedState {
    pub num_lock: bool,
    pub caps_lock: bool,
    pub scroll_lock: bool,
    pub compose: bool,
    pub kana: bool,
}

impl HostLedState {
    pub const fn from_bitmap(bitmap: u8) -> Self {
        Self {
            num_lock: bitmap & (1 << 0) != 0,
            caps_lock: bitmap & (1 << 1) != 0,
            scroll_lock: bitmap & (1 << 2) != 0,
            compose: bitmap & (1 << 3) != 0,
            kana: bitmap & (1 << 4) != 0,
        }
    }

    pub const fn bitmap(self) -> u8 {
        (self.num_lock as u8)
            | ((self.caps_lock as u8) << 1)
            | ((self.scroll_lock as u8) << 2)
            | ((self.compose as u8) << 3)
            | ((self.kana as u8) << 4)
    }
}
