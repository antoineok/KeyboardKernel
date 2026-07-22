use core::convert::TryFrom;

use super::{ManagementCommand, WireReader, WireWriteError, WireWriter};

pub const MANAGEMENT_MAGIC: [u8; 4] = *b"KWMP";
pub const MANAGEMENT_PROTOCOL_VERSION: u8 = 1;
pub const MANAGEMENT_HEADER_SIZE: usize = 16;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct FrameFlags(u8);

impl FrameFlags {
    pub const EMPTY: Self = Self(0);
    pub const RESPONSE: Self = Self(1 << 0);
    pub const ERROR: Self = Self(1 << 1);
    pub const MORE_FRAGMENTS: Self = Self(1 << 2);
    const KNOWN_MASK: u8 = Self::RESPONSE.0 | Self::ERROR.0 | Self::MORE_FRAGMENTS.0;

    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::KNOWN_MASK == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementHeader {
    pub version: u8,
    pub flags: FrameFlags,
    pub command: u16,
    pub request_id: u32,
    pub payload_length: u32,
}

impl ManagementHeader {
    pub const fn request(command: ManagementCommand, request_id: u32, payload_length: u32) -> Self {
        Self {
            version: MANAGEMENT_PROTOCOL_VERSION,
            flags: FrameFlags::EMPTY,
            command: command.value(),
            request_id,
            payload_length,
        }
    }

    pub const fn response(
        command: u16,
        request_id: u32,
        payload_length: u32,
        is_error: bool,
    ) -> Self {
        let mut flags = FrameFlags::RESPONSE;
        if is_error {
            flags = flags.union(FrameFlags::ERROR);
        }

        Self {
            version: MANAGEMENT_PROTOCOL_VERSION,
            flags,
            command,
            request_id,
            payload_length,
        }
    }

    pub fn encode(self, output: &mut [u8]) -> Result<(), WireWriteError> {
        if output.len() < MANAGEMENT_HEADER_SIZE {
            return Err(WireWriteError::BufferTooSmall);
        }

        let mut writer = WireWriter::new(&mut output[..MANAGEMENT_HEADER_SIZE]);
        writer.write_bytes(&MANAGEMENT_MAGIC)?;
        writer.write_u8(self.version)?;
        writer.write_u8(self.flags.bits())?;
        writer.write_u16(self.command)?;
        writer.write_u32(self.request_id)?;
        writer.write_u32(self.payload_length)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameDecodeError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u8),
    InvalidFlags(u8),
    PayloadLengthOverflow,
    LengthMismatch { declared: usize, actual: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementFrame<'a> {
    pub header: ManagementHeader,
    pub payload: &'a [u8],
}

impl<'a> ManagementFrame<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, FrameDecodeError> {
        if bytes.len() < MANAGEMENT_HEADER_SIZE {
            return Err(FrameDecodeError::TooShort);
        }

        let mut reader = WireReader::new(&bytes[..MANAGEMENT_HEADER_SIZE]);
        let magic = reader
            .read_bytes(4)
            .map_err(|_| FrameDecodeError::TooShort)?;

        if magic != MANAGEMENT_MAGIC {
            return Err(FrameDecodeError::BadMagic);
        }

        let version = reader.read_u8().map_err(|_| FrameDecodeError::TooShort)?;
        if version != MANAGEMENT_PROTOCOL_VERSION {
            return Err(FrameDecodeError::UnsupportedVersion(version));
        }

        let raw_flags = reader.read_u8().map_err(|_| FrameDecodeError::TooShort)?;
        let flags = FrameFlags::from_bits(raw_flags)
            .ok_or(FrameDecodeError::InvalidFlags(raw_flags))?;
        let command = reader.read_u16().map_err(|_| FrameDecodeError::TooShort)?;
        let request_id = reader.read_u32().map_err(|_| FrameDecodeError::TooShort)?;
        let payload_length = reader.read_u32().map_err(|_| FrameDecodeError::TooShort)?;

        let payload_length_usize = usize::try_from(payload_length)
            .map_err(|_| FrameDecodeError::PayloadLengthOverflow)?;
        let actual_payload_length = bytes.len() - MANAGEMENT_HEADER_SIZE;

        if payload_length_usize != actual_payload_length {
            return Err(FrameDecodeError::LengthMismatch {
                declared: payload_length_usize,
                actual: actual_payload_length,
            });
        }

        Ok(Self {
            header: ManagementHeader {
                version,
                flags,
                command,
                request_id,
                payload_length,
            },
            payload: &bytes[MANAGEMENT_HEADER_SIZE..],
        })
    }

    pub fn command(self) -> Result<ManagementCommand, ()> {
        ManagementCommand::try_from(self.header.command)
    }

    pub const fn is_response(self) -> bool {
        self.header.flags.contains(FrameFlags::RESPONSE)
    }
}
