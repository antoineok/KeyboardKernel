//! Minimal ISO Base Media File Format primitives used by the video subsystem.
//!
//! Keywork deliberately supports only a constrained MP4 profile. Structural
//! ISO-BMFF parsing stays separate from the Keywork-specific profile rules so
//! `moov`, `hvcC`, and fragmented `moof`/`mdat` parsing can share it.

use core::fmt;

use crate::storage::fs::vfs::{FileStream, Vfs, VfsError};

pub const BASIC_BOX_HEADER_SIZE: usize = 8;
pub const EXTENDED_BOX_HEADER_SIZE: usize = 16;

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct FourCc([u8; 4]);

impl FourCc {
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    #[inline(always)]
    pub const fn as_bytes(self) -> [u8; 4] {
        self.0
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}{}",
            printable_fourcc_byte(self.0[0]),
            printable_fourcc_byte(self.0[1]),
            printable_fourcc_byte(self.0[2]),
            printable_fourcc_byte(self.0[3]),
        )
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[inline(always)]
const fn printable_fourcc_byte(byte: u8) -> char {
    if byte >= 0x20 && byte <= 0x7e {
        byte as char
    } else {
        '?'
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoxHeader {
    box_type: FourCc,
    header_size: usize,
    box_size: usize,
}

impl BoxHeader {
    /// Parse one ISO-BMFF box header.
    ///
    /// `container_remaining` is the number of bytes from the beginning of this
    /// box to the end of its containing region. This resolves `size == 0` and
    /// prevents a box from escaping the enclosing file or parent box.
    pub fn parse(bytes: &[u8], container_remaining: usize) -> Result<Self, IsoBmffError> {
        if container_remaining < BASIC_BOX_HEADER_SIZE {
            return Err(IsoBmffError::TruncatedBoxHeader {
                available: container_remaining,
                required: BASIC_BOX_HEADER_SIZE,
            });
        }

        if bytes.len() < BASIC_BOX_HEADER_SIZE {
            return Err(IsoBmffError::TruncatedBoxHeader {
                available: bytes.len(),
                required: BASIC_BOX_HEADER_SIZE,
            });
        }

        let size32 = read_u32_be(bytes, 0);
        let box_type = FourCc::new([bytes[4], bytes[5], bytes[6], bytes[7]]);

        let (header_size, box_size) = match size32 {
            0 => (BASIC_BOX_HEADER_SIZE, container_remaining),
            1 => {
                if container_remaining < EXTENDED_BOX_HEADER_SIZE {
                    return Err(IsoBmffError::TruncatedBoxHeader {
                        available: container_remaining,
                        required: EXTENDED_BOX_HEADER_SIZE,
                    });
                }

                if bytes.len() < EXTENDED_BOX_HEADER_SIZE {
                    return Err(IsoBmffError::TruncatedBoxHeader {
                        available: bytes.len(),
                        required: EXTENDED_BOX_HEADER_SIZE,
                    });
                }

                let extended = read_u64_be(bytes, 8);
                let box_size = usize::try_from(extended)
                    .map_err(|_| IsoBmffError::BoxSizeOverflow(extended))?;
                (EXTENDED_BOX_HEADER_SIZE, box_size)
            }
            size => (BASIC_BOX_HEADER_SIZE, size as usize),
        };

        if box_size < header_size {
            return Err(IsoBmffError::InvalidBoxSize {
                box_type,
                declared: box_size,
                minimum: header_size,
            });
        }

        if box_size > container_remaining {
            return Err(IsoBmffError::BoxExceedsContainer {
                box_type,
                declared: box_size,
                available: container_remaining,
            });
        }

        Ok(Self {
            box_type,
            header_size,
            box_size,
        })
    }

    #[inline(always)]
    pub const fn box_type(self) -> FourCc {
        self.box_type
    }

    #[inline(always)]
    pub const fn header_size(self) -> usize {
        self.header_size
    }

    #[inline(always)]
    pub const fn box_size(self) -> usize {
        self.box_size
    }

    #[inline(always)]
    pub const fn payload_size(self) -> usize {
        self.box_size - self.header_size
    }
}

/// Sequential top-level box reader over a VFS stream.
///
/// No seeking is required. A fragmented MP4 can therefore be consumed directly
/// from FAT/eMMC as successive `moof`/`mdat` pairs are encountered.
pub struct StreamBoxReader<'a, 'storage> {
    vfs: &'a Vfs<'storage>,
    stream: &'a mut FileStream,
}

impl<'a, 'storage> StreamBoxReader<'a, 'storage> {
    pub fn new(vfs: &'a Vfs<'storage>, stream: &'a mut FileStream) -> Self {
        Self { vfs, stream }
    }

    pub fn next_header(&mut self) -> Result<Option<BoxHeader>, IsoBmffError> {
        let container_remaining = self.stream.remaining();

        if container_remaining == 0 {
            return Ok(None);
        }

        if container_remaining < BASIC_BOX_HEADER_SIZE {
            return Err(IsoBmffError::TruncatedBoxHeader {
                available: container_remaining,
                required: BASIC_BOX_HEADER_SIZE,
            });
        }

        let mut raw = [0u8; EXTENDED_BOX_HEADER_SIZE];
        self.vfs
            .read_exact_stream(self.stream, &mut raw[..BASIC_BOX_HEADER_SIZE])?;

        if read_u32_be(&raw, 0) == 1 {
            if container_remaining < EXTENDED_BOX_HEADER_SIZE {
                return Err(IsoBmffError::TruncatedBoxHeader {
                    available: container_remaining,
                    required: EXTENDED_BOX_HEADER_SIZE,
                });
            }

            self.vfs.read_exact_stream(
                self.stream,
                &mut raw[BASIC_BOX_HEADER_SIZE..EXTENDED_BOX_HEADER_SIZE],
            )?;
        }

        Ok(Some(BoxHeader::parse(&raw, container_remaining)?))
    }

    pub fn skip_payload(&mut self, header: BoxHeader) -> Result<(), IsoBmffError> {
        self.vfs.skip_stream(self.stream, header.payload_size())?;
        Ok(())
    }

    #[inline(always)]
    pub fn position(&self) -> usize {
        self.stream.position()
    }

    #[inline(always)]
    pub fn remaining(&self) -> usize {
        self.stream.remaining()
    }
}

#[derive(Debug)]
pub enum IsoBmffError {
    TruncatedBoxHeader {
        available: usize,
        required: usize,
    },
    BoxSizeOverflow(u64),
    InvalidBoxSize {
        box_type: FourCc,
        declared: usize,
        minimum: usize,
    },
    BoxExceedsContainer {
        box_type: FourCc,
        declared: usize,
        available: usize,
    },
    Vfs(VfsError),
}

impl From<VfsError> for IsoBmffError {
    fn from(error: VfsError) -> Self {
        Self::Vfs(error)
    }
}

impl fmt::Display for IsoBmffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedBoxHeader {
                available,
                required,
            } => {
                write!(
                    f,
                    "truncated ISO-BMFF box header: {available} bytes available, {required} required"
                )
            }
            Self::BoxSizeOverflow(size) => {
                write!(f, "ISO-BMFF box size {size} does not fit in usize")
            }
            Self::InvalidBoxSize {
                box_type,
                declared,
                minimum,
            } => {
                write!(
                    f,
                    "invalid {box_type} box size: declared {declared}, minimum is {minimum}"
                )
            }
            Self::BoxExceedsContainer {
                box_type,
                declared,
                available,
            } => {
                write!(
                    f,
                    "{box_type} box exceeds its container: declared {declared} bytes, {available} available"
                )
            }
            Self::Vfs(_) => write!(f, "VFS error while reading ISO-BMFF data"),
        }
    }
}

#[inline(always)]
fn read_u32_be(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

#[inline(always)]
fn read_u64_be(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}
