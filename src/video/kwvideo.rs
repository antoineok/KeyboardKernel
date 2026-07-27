//! Keywork Video (`kwv1`) profile validation.
//!
//! A Keywork Video file is still ordinary ISO-BMFF/MP4. The custom `kwv1`
//! major brand only promises that the file follows the constrained subset the
//! kernel supports.

use core::fmt;

use crate::storage::fs::vfs::{FileStream, Vfs, VfsError};

use super::isobmff::{FourCc, IsoBmffError, StreamBoxReader};

pub const FTYP_BOX: FourCc = FourCc::new(*b"ftyp");
pub const KWVIDEO_BRAND: FourCc = FourCc::new(*b"kwv1");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileType {
    major_brand: FourCc,
    minor_version: u32,
    compatible_brand_count: usize,
}

impl FileType {
    #[inline(always)]
    pub const fn major_brand(self) -> FourCc {
        self.major_brand
    }

    #[inline(always)]
    pub const fn minor_version(self) -> u32 {
        self.minor_version
    }

    #[inline(always)]
    pub const fn compatible_brand_count(self) -> usize {
        self.compatible_brand_count
    }
}

/// Validate the leading `ftyp` box of a Keywork Video stream.
///
/// The major brand carries the Keywork profile generation (`kwv1`). The MP4
/// minor-version field is retained as metadata but deliberately not used for
/// Keywork compatibility decisions; regular MP4 muxers choose that value.
pub fn parse_file_type(
    vfs: &Vfs<'_>,
    stream: &mut FileStream,
) -> Result<FileType, KwVideoError> {
    let mut boxes = StreamBoxReader::new(vfs, stream);
    let header = boxes.next_header()?.ok_or(KwVideoError::EmptyFile)?;
    drop(boxes);

    if header.box_type() != FTYP_BOX {
        return Err(KwVideoError::MissingFileType(header.box_type()));
    }

    let payload_size = header.payload_size();

    if payload_size < 8 {
        return Err(KwVideoError::InvalidFileTypeSize(payload_size));
    }

    if payload_size & 3 != 0 {
        return Err(KwVideoError::MisalignedFileTypeSize(payload_size));
    }

    let mut fixed = [0u8; 8];
    vfs.read_exact_stream(stream, &mut fixed)?;

    let major_brand = FourCc::new([fixed[0], fixed[1], fixed[2], fixed[3]]);
    let minor_version = u32::from_be_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]);

    if major_brand != KWVIDEO_BRAND {
        return Err(KwVideoError::UnsupportedMajorBrand(major_brand));
    }

    let compatible_brand_count = (payload_size - fixed.len()) / 4;

    for _ in 0..compatible_brand_count {
        let mut brand = [0u8; 4];
        vfs.read_exact_stream(stream, &mut brand)?;
    }

    Ok(FileType {
        major_brand,
        minor_version,
        compatible_brand_count,
    })
}

#[derive(Debug)]
pub enum KwVideoError {
    EmptyFile,
    MissingFileType(FourCc),
    InvalidFileTypeSize(usize),
    MisalignedFileTypeSize(usize),
    UnsupportedMajorBrand(FourCc),
    IsoBmff(IsoBmffError),
}

impl From<IsoBmffError> for KwVideoError {
    fn from(error: IsoBmffError) -> Self {
        Self::IsoBmff(error)
    }
}

impl From<VfsError> for KwVideoError {
    fn from(error: VfsError) -> Self {
        Self::IsoBmff(IsoBmffError::Vfs(error))
    }
}

impl fmt::Display for KwVideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFile => write!(f, "empty Keywork Video file"),
            Self::MissingFileType(found) => {
                write!(f, "Keywork Video must begin with ftyp, found {found}")
            }
            Self::InvalidFileTypeSize(size) => {
                write!(f, "Keywork Video ftyp payload is too small: {size} bytes")
            }
            Self::MisalignedFileTypeSize(size) => {
                write!(f, "Keywork Video ftyp payload is not a multiple of four: {size} bytes")
            }
            Self::UnsupportedMajorBrand(brand) => {
                write!(f, "unsupported video major brand {brand}; expected {KWVIDEO_BRAND}")
            }
            Self::IsoBmff(error) => fmt::Display::fmt(error, f),
        }
    }
}
