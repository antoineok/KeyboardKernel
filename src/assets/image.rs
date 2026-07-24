use core::fmt;

use crate::graphics::{
    blit_alpha, blit_opaque, BlendMode, BlitError, PixelFormat, Surface, SurfaceError, SurfaceInfo,
    SurfaceMut,
};
use crate::storage::fs::vfs::{FileStream, Vfs, VfsError};

pub const IMAGE_MAGIC: [u8; 4] = *b"KWIM";
pub const IMAGE_VERSION: u16 = 1;
pub const IMAGE_HEADER_SIZE: usize = 32;

const FORMAT_RGBX8888: u8 = 1;
const FORMAT_BGRX8888: u8 = 2;
const FORMAT_RGBA8888: u8 = 3;
const FORMAT_BGRA8888: u8 = 4;
const FORMAT_A8: u8 = 5;

/// Validated metadata from a native Keywork image header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHeader {
    header_size: usize,
    width: u32,
    height: u32,
    pitch: usize,
    data_len: usize,
    format: PixelFormat,
}

impl ImageHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self, ImageAssetError> {
        if bytes.len() < IMAGE_HEADER_SIZE {
            return Err(ImageAssetError::HeaderTooShort {
                available: bytes.len(),
                required: IMAGE_HEADER_SIZE,
            });
        }

        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];

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

        let flags = bytes[9];

        if flags != 0 {
            return Err(ImageAssetError::UnsupportedFlags(flags));
        }

        let format = decode_pixel_format(bytes[8])?;
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

        // Validate dimensions, pitch and format even when only the header is
        // resident. The streamed loader can then safely construct one-row
        // surface views from the same metadata.
        SurfaceInfo::new(width, height, pitch, format)?;

        Ok(Self {
            header_size,
            width,
            height,
            pitch,
            data_len,
            format,
        })
    }

    #[inline(always)]
    pub const fn header_size(&self) -> usize {
        self.header_size
    }

    #[inline(always)]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[inline(always)]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[inline(always)]
    pub const fn pitch(&self) -> usize {
        self.pitch
    }

    #[inline(always)]
    pub const fn data_len(&self) -> usize {
        self.data_len
    }

    #[inline(always)]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    #[inline(always)]
    pub fn encoded_len(&self) -> Result<usize, ImageAssetError> {
        self.header_size
            .checked_add(self.data_len)
            .ok_or(ImageAssetError::SizeOverflow)
    }
}

/// A zero-copy view over a native Keywork image already resident in memory.
pub struct ImageAsset<'a> {
    surface: Surface<'a>,
    encoded_len: usize,
}

impl<'a> ImageAsset<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ImageAssetError> {
        let header = ImageHeader::parse(bytes)?;
        let encoded_len = header.encoded_len()?;

        if bytes.len() < encoded_len {
            return Err(ImageAssetError::TruncatedData {
                available: bytes.len(),
                required: encoded_len,
            });
        }

        let info = SurfaceInfo::new(
            header.width(),
            header.height(),
            header.pitch(),
            header.format(),
        )?;

        let pixels = &bytes[header.header_size()..encoded_len];
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
    #[inline(always)]
    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

/// Stream a native image directly into a destination surface.
///
/// Only one source row must be resident at a time. This is intended for large
/// eMMC-backed assets such as fullscreen backgrounds; small cached assets can
/// continue to use `ImageAsset`.
#[allow(clippy::too_many_arguments)]
pub fn blit_streamed_image(
    vfs: &Vfs<'_>,
    stream: &mut FileStream,
    destination: &mut SurfaceMut<'_>,
    destination_x: i64,
    destination_y: i64,
    blend_mode: BlendMode,
    row_buffer: &mut [u8],
) -> Result<ImageHeader, StreamedImageError> {
    let asset_start = stream.position();
    let mut raw_header = [0u8; IMAGE_HEADER_SIZE];
    vfs.read_exact_stream(stream, &mut raw_header)?;

    let header = ImageHeader::parse(&raw_header)?;
    let encoded_len = header.encoded_len()?;
    let available = stream.len().saturating_sub(asset_start);

    if available < encoded_len {
        return Err(ImageAssetError::TruncatedData {
            available,
            required: encoded_len,
        }
        .into());
    }

    if header.pitch() > row_buffer.len() {
        return Err(StreamedImageError::RowBufferTooSmall {
            available: row_buffer.len(),
            required: header.pitch(),
        });
    }

    let extended_header_size = header.header_size() - IMAGE_HEADER_SIZE;

    if extended_header_size != 0 {
        vfs.skip_stream(stream, extended_header_size)?;
    }

    let row_info = SurfaceInfo::new(header.width(), 1, header.pitch(), header.format())?;

    for source_y in 0..header.height() {
        let row_bytes = &mut row_buffer[..header.pitch()];
        vfs.read_exact_stream(stream, row_bytes)?;
        let source = Surface::new(row_bytes, row_info)?;
        let row_y = destination_y + i64::from(source_y);

        match blend_mode {
            BlendMode::Opaque => blit_opaque(
                &source,
                destination,
                0,
                0,
                destination_x,
                row_y,
                header.width(),
                1,
            )?,
            BlendMode::Alpha => blit_alpha(
                &source,
                destination,
                0,
                0,
                destination_x,
                row_y,
                header.width(),
                1,
            )?,
        }
    }

    Ok(header)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageAssetError {
    HeaderTooShort { available: usize, required: usize },

    InvalidMagic([u8; 4]),

    UnsupportedVersion(u16),

    InvalidHeaderSize { declared: usize, minimum: usize },

    UnsupportedFlags(u8),

    UnknownPixelFormat(u8),

    InvalidDataLength { declared: usize, required: usize },

    TruncatedData { available: usize, required: usize },

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

            Self::InvalidHeaderSize { declared, minimum } => {
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

            Self::InvalidDataLength { declared, required } => {
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

#[derive(Debug)]
pub enum StreamedImageError {
    Vfs(VfsError),
    Image(ImageAssetError),
    Surface(SurfaceError),
    Blit(BlitError),
    RowBufferTooSmall { available: usize, required: usize },
}

impl From<VfsError> for StreamedImageError {
    fn from(error: VfsError) -> Self {
        Self::Vfs(error)
    }
}

impl From<ImageAssetError> for StreamedImageError {
    fn from(error: ImageAssetError) -> Self {
        Self::Image(error)
    }
}

impl From<SurfaceError> for StreamedImageError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl From<BlitError> for StreamedImageError {
    fn from(error: BlitError) -> Self {
        Self::Blit(error)
    }
}

impl fmt::Display for StreamedImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vfs(error) => write!(f, "image stream I/O failed: {error:?}"),
            Self::Image(error) => write!(f, "invalid streamed image: {error}"),
            Self::Surface(error) => write!(f, "invalid streamed image row: {error}"),
            Self::Blit(error) => write!(f, "streamed image blit failed: {error}"),
            Self::RowBufferTooSmall {
                available,
                required,
            } => write!(
                f,
                "streamed image row buffer is too small: {available} bytes available, \
                 {required} required"
            ),
        }
    }
}

fn decode_pixel_format(format: u8) -> Result<PixelFormat, ImageAssetError> {
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
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
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
