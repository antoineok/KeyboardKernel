use core::fmt;

use crate::graphics::{
    PixelFormat,
    Surface,
    SurfaceError,
    SurfaceInfo,
};

pub const IMAGE_MAGIC: [u8; 4] = *b"KWIM";
pub const IMAGE_VERSION: u16 = 1;
pub const IMAGE_HEADER_SIZE: usize = 32;

const FORMAT_RGBX8888: u8 = 1;
const FORMAT_BGRX8888: u8 = 2;
const FORMAT_RGBA8888: u8 = 3;
const FORMAT_BGRA8888: u8 = 4;
const FORMAT_A8: u8 = 5;

/// A zero-copy view over a native Keywork image asset.
pub struct ImageAsset<'a> {
    surface: Surface<'a>,
    encoded_len: usize,
}

impl<'a> ImageAsset<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ImageAssetError> {
        if bytes.len() < IMAGE_HEADER_SIZE {
            return Err(ImageAssetError::HeaderTooShort {
                available: bytes.len(),
                required: IMAGE_HEADER_SIZE,
            });
        }

        let magic = [
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
        ];

        if magic != IMAGE_MAGIC {
            return Err(ImageAssetError::InvalidMagic(magic));
        }

        let version = read_u16_le(bytes, 4);

        if version != IMAGE_VERSION {
            return Err(ImageAssetError::UnsupportedVersion(version));
        }

        let header_size = read_u16_le(bytes, 6) as usize;

        if header_size < IMAGE_HEADER_SIZE {
            return Err(ImageAssetError::InvalidHeaderSize {
                declared: header_size,
                minimum: IMAGE_HEADER_SIZE,
            });
        }

        let format_code = bytes[8];
        let flags = bytes[9];

        if flags != 0 {
            return Err(ImageAssetError::UnsupportedFlags(flags));
        }

        let format = decode_pixel_format(format_code)?;

        let width = read_u32_le(bytes, 12);
        let height = read_u32_le(bytes, 16);
        let pitch = read_u32_le(bytes, 20) as usize;
        let data_len = read_u32_le(bytes, 24) as usize;

        let required_data_len = pitch
            .checked_mul(height as usize)
            .ok_or(ImageAssetError::SizeOverflow)?;

        if data_len != required_data_len {
            return Err(ImageAssetError::InvalidDataLength {
                declared: data_len,
                required: required_data_len,
            });
        }

        let encoded_len = header_size
            .checked_add(data_len)
            .ok_or(ImageAssetError::SizeOverflow)?;

        if bytes.len() < encoded_len {
            return Err(ImageAssetError::TruncatedData {
                available: bytes.len(),
                required: encoded_len,
            });
        }

        let info = SurfaceInfo::new(
            width,
            height,
            pitch,
            format,
        )?;

        let pixels = &bytes[header_size..encoded_len];
        let surface = Surface::new(pixels, info)?;

        Ok(Self {
            surface,
            encoded_len,
        })
    }

    #[inline(always)]
    pub const fn surface(&self) -> Surface<'a> {
        self.surface
    }

    #[inline(always)]
    pub const fn width(&self) -> u32 {
        self.surface.width()
    }

    #[inline(always)]
    pub const fn height(&self) -> u32 {
        self.surface.height()
    }

    #[inline(always)]
    pub const fn pitch(&self) -> usize {
        self.surface.pitch()
    }

    #[inline(always)]
    pub const fn format(&self) -> PixelFormat {
        self.surface.format()
    }

    /// Number of bytes occupied by this encoded image.
    ///
    /// This allows a future asset-pack parser to advance to the next asset.
    #[inline(always)]
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageAssetError {
    HeaderTooShort {
        available: usize,
        required: usize,
    },

    InvalidMagic([u8; 4]),

    UnsupportedVersion(u16),

    InvalidHeaderSize {
        declared: usize,
        minimum: usize,
    },

    UnsupportedFlags(u8),

    UnknownPixelFormat(u8),

    InvalidDataLength {
        declared: usize,
        required: usize,
    },

    TruncatedData {
        available: usize,
        required: usize,
    },

    SizeOverflow,

    Surface(SurfaceError),
}

impl From<SurfaceError> for ImageAssetError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl fmt::Display for ImageAssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooShort {
                available,
                required,
            } => {
                write!(
                    f,
                    "image header is too short: {available} bytes available, \
                     {required} required"
                )
            }

            Self::InvalidMagic(magic) => {
                write!(f, "invalid image magic: {magic:02x?}")
            }

            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported image version: {version}")
            }

            Self::InvalidHeaderSize {
                declared,
                minimum,
            } => {
                write!(
                    f,
                    "invalid image header size: {declared}, minimum is {minimum}"
                )
            }

            Self::UnsupportedFlags(flags) => {
                write!(f, "unsupported image flags: {flags:#04x}")
            }

            Self::UnknownPixelFormat(format) => {
                write!(f, "unknown image pixel format: {format}")
            }

            Self::InvalidDataLength {
                declared,
                required,
            } => {
                write!(
                    f,
                    "invalid image data length: declared {declared}, \
                     expected {required}"
                )
            }

            Self::TruncatedData {
                available,
                required,
            } => {
                write!(
                    f,
                    "truncated image asset: {available} bytes available, \
                     {required} required"
                )
            }

            Self::SizeOverflow => {
                write!(f, "image size calculation overflowed")
            }

            Self::Surface(error) => {
                write!(f, "invalid image surface: {error}")
            }
        }
    }
}

fn decode_pixel_format(
    format: u8,
) -> Result<PixelFormat, ImageAssetError> {
    match format {
        FORMAT_RGBX8888 => Ok(PixelFormat::Rgbx8888),
        FORMAT_BGRX8888 => Ok(PixelFormat::Bgrx8888),
        FORMAT_RGBA8888 => Ok(PixelFormat::Rgba8888),
        FORMAT_BGRA8888 => Ok(PixelFormat::Bgra8888),
        FORMAT_A8 => Ok(PixelFormat::A8),

        _ => Err(ImageAssetError::UnknownPixelFormat(format)),
    }
}

#[inline(always)]
fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
    ])
}

#[inline(always)]
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}