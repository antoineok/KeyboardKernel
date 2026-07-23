//! USB descriptors for the Keywork composite device.
//!
//! Interface 0 is a boot-protocol keyboard, interface 1 carries consumer and
//! system-control reports, and interface 2 is the vendor-specific management
//! channel. Management uses bulk endpoints instead of pretending to be HID.

pub const DEVICE_DESCRIPTOR_TYPE: u8 = 0x01;
pub const CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x02;
pub const STRING_DESCRIPTOR_TYPE: u8 = 0x03;
pub const INTERFACE_DESCRIPTOR_TYPE: u8 = 0x04;
pub const ENDPOINT_DESCRIPTOR_TYPE: u8 = 0x05;
pub const DEVICE_QUALIFIER_DESCRIPTOR_TYPE: u8 = 0x06;
pub const OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x07;
pub const HID_DESCRIPTOR_TYPE: u8 = 0x21;
pub const HID_REPORT_DESCRIPTOR_TYPE: u8 = 0x22;

pub const KEYBOARD_INTERFACE: u8 = 0;
pub const EXTRA_HID_INTERFACE: u8 = 1;
pub const MANAGEMENT_INTERFACE: u8 = 2;

pub const KEYBOARD_ENDPOINT: u8 = 1;
pub const MANAGEMENT_ENDPOINT: u8 = 2;
pub const EXTRA_HID_ENDPOINT: u8 = 3;

pub const KEYBOARD_ENDPOINT_ADDRESS: u8 = 0x80 | KEYBOARD_ENDPOINT;
pub const MANAGEMENT_OUT_ENDPOINT_ADDRESS: u8 = MANAGEMENT_ENDPOINT;
pub const MANAGEMENT_IN_ENDPOINT_ADDRESS: u8 = 0x80 | MANAGEMENT_ENDPOINT;
pub const EXTRA_HID_ENDPOINT_ADDRESS: u8 = 0x80 | EXTRA_HID_ENDPOINT;

pub const KEYBOARD_MAX_PACKET_SIZE: u16 = 8;
pub const EXTRA_HID_MAX_PACKET_SIZE: u16 = 3;
pub const MANAGEMENT_HIGH_SPEED_MAX_PACKET_SIZE: u16 = 512;
pub const MANAGEMENT_FULL_SPEED_MAX_PACKET_SIZE: u16 = 64;

/// Development-only VID/PID pair.
///
/// Do not ship a product with this pair. Replace it with an assigned VID/PID
/// before distributing hardware.
pub const DEVICE_DESCRIPTOR: [u8; 18] = [
    18,
    DEVICE_DESCRIPTOR_TYPE,
    0x00,
    0x02, // USB 2.0
    0x00, // class defined per interface
    0x00,
    0x00,
    64, // EP0 packet size
    0xfe,
    0xca, // VID 0xCAFE
    0x01,
    0x40, // PID 0x4001
    0x00,
    0x01, // device 1.00
    1,    // manufacturer
    2,    // product
    3,    // serial
    1,    // configurations
];

/// Standard boot-keyboard report descriptor.
pub const KEYBOARD_REPORT_DESCRIPTOR: [u8; 63] = [
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xa1, 0x01, // Collection (Application)
    0x05, 0x07, // Usage Page (Keyboard)
    0x19, 0xe0, // Usage Minimum (Left Control)
    0x29, 0xe7, // Usage Maximum (Right GUI)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x01, // Logical Maximum (1)
    0x75, 0x01, // Report Size (1)
    0x95, 0x08, // Report Count (8)
    0x81, 0x02, // Input (Data, Variable, Absolute)
    0x95, 0x01, // Report Count (1)
    0x75, 0x08, // Report Size (8)
    0x81, 0x01, // Input (Constant)
    0x95, 0x05, // Report Count (5)
    0x75, 0x01, // Report Size (1)
    0x05, 0x08, // Usage Page (LEDs)
    0x19, 0x01, // Usage Minimum (Num Lock)
    0x29, 0x05, // Usage Maximum (Kana)
    0x91, 0x02, // Output (Data, Variable, Absolute)
    0x95, 0x01, // Report Count (1)
    0x75, 0x03, // Report Size (3)
    0x91, 0x01, // Output (Constant)
    0x95, 0x06, // Report Count (6)
    0x75, 0x08, // Report Size (8)
    0x15, 0x00, // Logical Minimum (0)
    0x25, 0x65, // Logical Maximum (101)
    0x05, 0x07, // Usage Page (Keyboard)
    0x19, 0x00, // Usage Minimum (Reserved)
    0x29, 0x65, // Usage Maximum (Keyboard Application)
    0x81, 0x00, // Input (Data, Array, Absolute)
    0xc0, // End Collection
];

pub const EXTRA_REPORT_ID_CONSUMER: u8 = 1;
pub const EXTRA_REPORT_ID_SYSTEM: u8 = 2;

/// Consumer-control and system-control reports share one HID interface using
/// report IDs. The keyboard interface deliberately has no report ID so it
/// remains boot-protocol compatible.
pub const EXTRA_HID_REPORT_DESCRIPTOR: [u8; 49] = [
    // Report ID 1: one 16-bit Consumer usage selector.
    0x05,
    0x0c, // Usage Page (Consumer)
    0x09,
    0x01, // Usage (Consumer Control)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    EXTRA_REPORT_ID_CONSUMER,
    0x15,
    0x00, // Logical Minimum (0 = released)
    0x26,
    0xff,
    0x03, // Logical Maximum (0x03ff)
    0x19,
    0x00, // Usage Minimum
    0x2a,
    0xff,
    0x03, // Usage Maximum
    0x75,
    0x10, // Report Size (16)
    0x95,
    0x01, // Report Count (1)
    0x81,
    0x00, // Input (Data, Array, Absolute)
    0xc0, // End Collection
    // Report ID 2: one 8-bit System Control usage selector.
    0x05,
    0x01, // Usage Page (Generic Desktop)
    0x09,
    0x80, // Usage (System Control)
    0xa1,
    0x01, // Collection (Application)
    0x85,
    EXTRA_REPORT_ID_SYSTEM,
    0x15,
    0x00, // Logical Minimum (0 = released)
    0x26,
    0x83,
    0x00, // Logical Maximum (System Wake Up)
    0x19,
    0x00, // Usage Minimum
    0x29,
    0x83, // Usage Maximum
    0x75,
    0x08, // Report Size (8)
    0x95,
    0x01, // Report Count (1)
    0x81,
    0x00, // Input (Data, Array, Absolute)
    0xc0, // End Collection
];

pub const KEYBOARD_HID_DESCRIPTOR: [u8; 9] = hid_descriptor(KEYBOARD_REPORT_DESCRIPTOR.len());
pub const EXTRA_HID_DESCRIPTOR: [u8; 9] = hid_descriptor(EXTRA_HID_REPORT_DESCRIPTOR.len());

const fn hid_descriptor(report_length: usize) -> [u8; 9] {
    [
        9,
        HID_DESCRIPTOR_TYPE,
        0x11,
        0x01, // HID 1.11
        0,
        1,
        HID_REPORT_DESCRIPTOR_TYPE,
        report_length as u8,
        (report_length >> 8) as u8,
    ]
}

const CONFIGURATION_TOTAL_LENGTH: u16 = 82;

/// High-speed descriptor: management bulk endpoints use 512-byte packets.
pub const CONFIGURATION_DESCRIPTOR_HIGH_SPEED: [u8; CONFIGURATION_TOTAL_LENGTH as usize] =
    configuration_descriptor(
        CONFIGURATION_DESCRIPTOR_TYPE,
        MANAGEMENT_HIGH_SPEED_MAX_PACKET_SIZE,
        4,
    );

/// Full-speed descriptor: management bulk endpoints use 64-byte packets.
pub const CONFIGURATION_DESCRIPTOR_FULL_SPEED: [u8; CONFIGURATION_TOTAL_LENGTH as usize] =
    configuration_descriptor(
        CONFIGURATION_DESCRIPTOR_TYPE,
        MANAGEMENT_FULL_SPEED_MAX_PACKET_SIZE,
        10,
    );

pub const OTHER_SPEED_CONFIGURATION_DESCRIPTOR_HIGH_SPEED: [u8; CONFIGURATION_TOTAL_LENGTH
    as usize] = configuration_descriptor(
    OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE,
    MANAGEMENT_HIGH_SPEED_MAX_PACKET_SIZE,
    4,
);

pub const OTHER_SPEED_CONFIGURATION_DESCRIPTOR_FULL_SPEED: [u8; CONFIGURATION_TOTAL_LENGTH
    as usize] = configuration_descriptor(
    OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE,
    MANAGEMENT_FULL_SPEED_MAX_PACKET_SIZE,
    10,
);

const fn configuration_descriptor(
    descriptor_type: u8,
    management_packet_size: u16,
    hid_interval: u8,
) -> [u8; CONFIGURATION_TOTAL_LENGTH as usize] {
    let mgmt_lo = management_packet_size as u8;
    let mgmt_hi = (management_packet_size >> 8) as u8;

    [
        // Configuration.
        9,
        descriptor_type,
        CONFIGURATION_TOTAL_LENGTH as u8,
        (CONFIGURATION_TOTAL_LENGTH >> 8) as u8,
        3, // interfaces
        1, // configuration value
        0,
        0x80, // bus powered
        50,   // 100 mA
        // Interface 0: boot keyboard.
        9,
        INTERFACE_DESCRIPTOR_TYPE,
        KEYBOARD_INTERFACE,
        0,
        1,
        0x03, // HID
        0x01, // boot interface
        0x01, // keyboard
        0,
        // Keyboard HID descriptor.
        KEYBOARD_HID_DESCRIPTOR[0],
        KEYBOARD_HID_DESCRIPTOR[1],
        KEYBOARD_HID_DESCRIPTOR[2],
        KEYBOARD_HID_DESCRIPTOR[3],
        KEYBOARD_HID_DESCRIPTOR[4],
        KEYBOARD_HID_DESCRIPTOR[5],
        KEYBOARD_HID_DESCRIPTOR[6],
        KEYBOARD_HID_DESCRIPTOR[7],
        KEYBOARD_HID_DESCRIPTOR[8],
        // Endpoint 0x81: keyboard interrupt IN.
        7,
        ENDPOINT_DESCRIPTOR_TYPE,
        KEYBOARD_ENDPOINT_ADDRESS,
        0x03,
        KEYBOARD_MAX_PACKET_SIZE as u8,
        (KEYBOARD_MAX_PACKET_SIZE >> 8) as u8,
        hid_interval,
        // Interface 1: consumer/system controls.
        9,
        INTERFACE_DESCRIPTOR_TYPE,
        EXTRA_HID_INTERFACE,
        0,
        1,
        0x03, // HID
        0x00,
        0x00,
        0,
        // Extra HID descriptor.
        EXTRA_HID_DESCRIPTOR[0],
        EXTRA_HID_DESCRIPTOR[1],
        EXTRA_HID_DESCRIPTOR[2],
        EXTRA_HID_DESCRIPTOR[3],
        EXTRA_HID_DESCRIPTOR[4],
        EXTRA_HID_DESCRIPTOR[5],
        EXTRA_HID_DESCRIPTOR[6],
        EXTRA_HID_DESCRIPTOR[7],
        EXTRA_HID_DESCRIPTOR[8],
        // Endpoint 0x83: extra HID interrupt IN.
        7,
        ENDPOINT_DESCRIPTOR_TYPE,
        EXTRA_HID_ENDPOINT_ADDRESS,
        0x03,
        EXTRA_HID_MAX_PACKET_SIZE as u8,
        (EXTRA_HID_MAX_PACKET_SIZE >> 8) as u8,
        hid_interval,
        // Interface 2: Keywork Management Protocol.
        9,
        INTERFACE_DESCRIPTOR_TYPE,
        MANAGEMENT_INTERFACE,
        0,
        2,
        0xff, // vendor specific
        0x4b, // 'K'
        0x01, // KWMP v1
        0,
        // Endpoint 0x02: management bulk OUT.
        7,
        ENDPOINT_DESCRIPTOR_TYPE,
        MANAGEMENT_OUT_ENDPOINT_ADDRESS,
        0x02,
        mgmt_lo,
        mgmt_hi,
        0,
        // Endpoint 0x82: management bulk IN.
        7,
        ENDPOINT_DESCRIPTOR_TYPE,
        MANAGEMENT_IN_ENDPOINT_ADDRESS,
        0x02,
        mgmt_lo,
        mgmt_hi,
        0,
    ]
}

pub const DEVICE_QUALIFIER_DESCRIPTOR: [u8; 10] = [
    10,
    DEVICE_QUALIFIER_DESCRIPTOR_TYPE,
    0x00,
    0x02,
    0,
    0,
    0,
    64,
    1,
    0,
];

pub const STRING_LANGUAGES: [u8; 4] = [4, STRING_DESCRIPTOR_TYPE, 0x09, 0x04];

pub const STRING_MANUFACTURER: [u8; 16] = [
    16,
    STRING_DESCRIPTOR_TYPE,
    b'K',
    0,
    b'e',
    0,
    b'y',
    0,
    b'w',
    0,
    b'o',
    0,
    b'r',
    0,
    b'k',
    0,
];

pub const STRING_PRODUCT: [u8; 34] = [
    34,
    STRING_DESCRIPTOR_TYPE,
    b'K',
    0,
    b'e',
    0,
    b'y',
    0,
    b'w',
    0,
    b'o',
    0,
    b'r',
    0,
    b'k',
    0,
    b' ',
    0,
    b'K',
    0,
    b'e',
    0,
    b'y',
    0,
    b'b',
    0,
    b'o',
    0,
    b'a',
    0,
    b'r',
    0,
    b'd',
    0,
];

pub const STRING_SERIAL: [u8; 16] = [
    16,
    STRING_DESCRIPTOR_TYPE,
    b'K',
    0,
    b'W',
    0,
    b'-',
    0,
    b'0',
    0,
    b'0',
    0,
    b'0',
    0,
    b'1',
    0,
];

pub fn string_descriptor(index: u8, language_id: u16) -> Option<&'static [u8]> {
    match index {
        0 => Some(&STRING_LANGUAGES),
        1 if language_id == 0x0409 => Some(&STRING_MANUFACTURER),
        2 if language_id == 0x0409 => Some(&STRING_PRODUCT),
        3 if language_id == 0x0409 => Some(&STRING_SERIAL),
        _ => None,
    }
}
