//! Minimal descriptors used to validate USB device enumeration.
//!
//! This is intentionally not the final keyboard descriptor set. It exposes a
//! single vendor-specific interface with no non-control endpoints, which is
//! enough to validate endpoint zero and the standard enumeration requests.

pub const DEVICE_DESCRIPTOR_TYPE: u8 = 0x01;
pub const CONFIGURATION_DESCRIPTOR_TYPE: u8 = 0x02;
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
    0,                          // iManufacturer
    0,                          // iProduct
    0,                          // iSerialNumber
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
