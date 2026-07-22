/// Status stored at the beginning of every KWMP response payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ManagementStatus {
    Ok = 0x0000,

    UnknownCommand = 0x0001,
    InvalidPayload = 0x0002,
    InvalidState = 0x0003,
    Unsupported = 0x0004,

    Busy = 0x0010,
    Timeout = 0x0011,

    NotFound = 0x0020,
    AlreadyExists = 0x0021,
    NoSpace = 0x0022,
    PermissionDenied = 0x0023,

    InvalidTransfer = 0x0030,
    InvalidOffset = 0x0031,
    ChecksumMismatch = 0x0032,

    InvalidFirmware = 0x0040,
    InvalidSignature = 0x0041,
    VersionRejected = 0x0042,

    InternalError = 0xffff,
}

impl ManagementStatus {
    pub const fn value(self) -> u16 {
        self as u16
    }

    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}
