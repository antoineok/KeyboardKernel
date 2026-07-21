//! Minimal descriptors used to validate USB device enumeration.
//!
//! This is intentionally not the final keyboard descriptor set. It exposes a
//! single vendor-specific interface with no non-control endpoints, which is
//! enough to validate endpoint zero and the standard enumeration requests.

pub const DEVICE_DESCRIPTOR_TYPE: u8 = 0x01;
pub const CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x02;
pub const STRING_DESCRIPTOR_TYPE: u8 = 0x03;
pub const DEVICE_QUALIFIER_DESCRIPTOR_TYPE: u8 = 0x06;
pub const OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x07;

/// Development-only VID/PID pair.
///
/// Do not ship a product with this pair. Replace it with an assigned VID/PID
/// before distributing hardware.
pub const DEVICE_DESCRIPTOR: [u8; 18] = [
    18,                         // bLength
    DEVICE_DESCRIPTOR_TYPE,     // bDescriptorType
    0x00, 0x02,                 // bcdUSB = USB 2.0
    0x00,                       // bDeviceClass (defined per interface)
    0x00,                       // bDeviceSubClass
    0x00,                       // bDeviceProtocol
    64,                         // bMaxPacketSize0
    0xfe, 0xca,                 // idVendor = 0xCAFE (development only)
    0x01, 0x40,                 // idProduct = 0x4001
    0x00, 0x01,                 // bcdDevice = 1.00
    1,                          // iManufacturer: "Keywork"
    2,                          // iProduct: "Keywork Keyboard"
    3,                          // iSerialNumber: "KW-0001"
    1,                          // bNumConfigurations
];

/// One configuration containing one vendor-specific interface and no
/// non-control endpoints.
pub const CONFIGURATION_DESCRIPTOR: [u8; 18] = [
    // Configuration descriptor.
    9,                              // bLength
    CONFIGURATION_DESCRIPTOR_TYPE,  // bDescriptorType
    18, 0,                          // wTotalLength
    1,                              // bNumInterfaces
    1,                              // bConfigurationValue
    0,                              // iConfiguration
    0x80,                           // bmAttributes: bus powered
    50,                             // bMaxPower: 100 mA

    // Interface descriptor.
    9,      // bLength
    4,      // bDescriptorType = INTERFACE
    0,      // bInterfaceNumber
    0,      // bAlternateSetting
    0,      // bNumEndpoints (EP0 is implicit)
    0xff,   // bInterfaceClass = vendor specific
    0x00,   // bInterfaceSubClass
    0x00,   // bInterfaceProtocol
    0,      // iInterface
];

/// Device qualifier returned when a high-speed host asks about operation at
/// the other USB speed.
pub const DEVICE_QUALIFIER_DESCRIPTOR: [u8; 10] = [
    10,
    DEVICE_QUALIFIER_DESCRIPTOR_TYPE,
    0x00, 0x02, // USB 2.0
    0x00,
    0x00,
    0x00,
    64,
    1,          // one configuration at the other speed
    0,
];

/// The current minimal configuration is valid at both full and high speed.
pub const OTHER_SPEED_CONFIGURATION_DESCRIPTOR: [u8; 18] = [
    9,
    OTHER_SPEED_CONFIGURATION_DESCRIPTOR_TYPE,
    18, 0,
    1,
    1,
    0,
    0x80,
    50,
    9,
    4,
    0,
    0,
    0,
    0xff,
    0x00,
    0x00,
    0,
];


/// String descriptor index 0: supported language IDs.
///
/// 0x0409 is English (United States). Unlike the other string descriptors,
/// this descriptor is requested with `wIndex == 0`.
pub const STRING_LANGUAGES: [u8; 4] = [
    4,
    STRING_DESCRIPTOR_TYPE,
    0x09,
    0x04,
];

/// Manufacturer string: "Keywork".
pub const STRING_MANUFACTURER: [u8; 16] = [
    16,
    STRING_DESCRIPTOR_TYPE,
    b'K', 0,
    b'e', 0,
    b'y', 0,
    b'w', 0,
    b'o', 0,
    b'r', 0,
    b'k', 0,
];

/// Product string: "Keywork Keyboard".
pub const STRING_PRODUCT: [u8; 34] = [
    34,
    STRING_DESCRIPTOR_TYPE,
    b'K', 0,
    b'e', 0,
    b'y', 0,
    b'w', 0,
    b'o', 0,
    b'r', 0,
    b'k', 0,
    b' ', 0,
    b'K', 0,
    b'e', 0,
    b'y', 0,
    b'b', 0,
    b'o', 0,
    b'a', 0,
    b'r', 0,
    b'd', 0,
];

/// Temporary development serial string: "KW-0001".
///
/// Replace this with a stable per-device value later. It must not change on
/// every boot, otherwise hosts will treat the same board as a new USB device.
pub const STRING_SERIAL: [u8; 16] = [
    16,
    STRING_DESCRIPTOR_TYPE,
    b'K', 0,
    b'W', 0,
    b'-', 0,
    b'0', 0,
    b'0', 0,
    b'0', 0,
    b'1', 0,
];

/// Returns a string descriptor by index.
///
/// Index zero returns the supported-language table. Text strings are currently
/// available only for English (United States), language ID 0x0409.
pub fn string_descriptor(index: u8, language_id: u16) -> Option<&'static [u8]> {
    match index {
        0 => Some(&STRING_LANGUAGES),
        1 if language_id == 0x0409 => Some(&STRING_MANUFACTURER),
        2 if language_id == 0x0409 => Some(&STRING_PRODUCT),
        3 if language_id == 0x0409 => Some(&STRING_SERIAL),
        _ => None,
    }
}
