use core::fmt;

/// Pixel storage format used by framebuffers, images, masks and other
/// renderable surfaces.
///
/// Variant names describe byte order in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgbx8888, // Opaque framebuffer
    Bgrx8888,

    Rgba8888, // Transparent image/button/icon
    Bgra8888,

    A8,       // Font glyph or mask

    None,     // Uninitialized framebuffer
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgbx8888 | Self::Bgrx8888 => 4,
            Self::Rgba8888 | Self::Bgra8888 => 4,
            Self::A8 => 1,
            Self::None => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceInfo {
    width: u32,
    height: u32,
    pitch: usize,
    format: PixelFormat,
}

impl SurfaceInfo {
    pub fn new(
        width: u32,
        height: u32,
        pitch: usize,
        format: PixelFormat,
    ) -> Result<Self, SurfaceError> {
        if format == PixelFormat::None {
            return Err(SurfaceError::UninitializedFormat);
        }

        if width == 0 || height == 0 {
            return Err(SurfaceError::EmptySurface);
        }

        let visible_row_bytes = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or(SurfaceError::SizeOverflow)?;

        if pitch < visible_row_bytes {
            return Err(SurfaceError::PitchTooSmall {
                pitch,
                minimum: visible_row_bytes,
            });
        }

        pitch
            .checked_mul(height as usize)
            .ok_or(SurfaceError::SizeOverflow)?;

        Ok(Self {
            width,
            height,
            pitch,
            format,
        })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub const fn pitch(self) -> usize {
        self.pitch
    }

    pub const fn format(self) -> PixelFormat {
        self.format
    }

    pub const fn bytes_per_pixel(self) -> usize {
        self.format.bytes_per_pixel()
    }

    pub fn visible_row_bytes(self) -> usize {
        self.width as usize * self.bytes_per_pixel()
    }

    pub fn required_buffer_size(self) -> Result<usize, SurfaceError> {
        self.pitch
            .checked_mul(self.height as usize)
            .ok_or(SurfaceError::SizeOverflow)
    }
}

/// Immutable pixel surface.
///
/// This will later be suitable for image assets, font atlases and decoded
/// video frames used as blit sources.
pub struct Surface<'a> {
    pixels: &'a [u8],
    info: SurfaceInfo,
}

impl<'a> Surface<'a> {
    pub fn new(pixels: &'a [u8], info: SurfaceInfo) -> Result<Self, SurfaceError> {
        validate_buffer(pixels.len(), info)?;

        Ok(Self { pixels, info })
    }

    pub const fn info(&self) -> SurfaceInfo {
        self.info
    }

    pub const fn width(&self) -> u32 {
        self.info.width()
    }

    pub const fn height(&self) -> u32 {
        self.info.height()
    }

    pub const fn pitch(&self) -> usize {
        self.info.pitch()
    }

    pub const fn format(&self) -> PixelFormat {
        self.info.format()
    }

    pub const fn pixels(&self) -> &[u8] {
        self.pixels
    }

    /// Returns the visible pixels of a row, excluding pitch padding.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height() {
            return None;
        }

        let start = y as usize * self.pitch();
        let end = start + self.info.visible_row_bytes();

        self.pixels.get(start..end)
    }
}

/// Mutable pixel surface.
///
/// The mailbox framebuffer and future off-screen backbuffers use this type.
pub struct SurfaceMut<'a> {
    pixels: &'a mut [u8],
    info: SurfaceInfo,
}

impl<'a> SurfaceMut<'a> {
    pub fn new(
        pixels: &'a mut [u8],
        info: SurfaceInfo,
    ) -> Result<Self, SurfaceError> {
        validate_buffer(pixels.len(), info)?;

        Ok(Self { pixels, info })
    }

    pub const fn info(&self) -> SurfaceInfo {
        self.info
    }

    pub const fn width(&self) -> u32 {
        self.info.width()
    }

    pub const fn height(&self) -> u32 {
        self.info.height()
    }

    pub const fn pitch(&self) -> usize {
        self.info.pitch()
    }

    pub const fn format(&self) -> PixelFormat {
        self.info.format()
    }

    pub fn pixels(&self) -> &[u8] {
        self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        self.pixels
    }

    pub fn as_surface(&self) -> Surface<'_> {
        Surface {
            pixels: &*self.pixels,
            info: self.info,
        }
    }

    /// Returns the visible pixels of a row, excluding pitch padding.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height() {
            return None;
        }

        let start = y as usize * self.pitch();
        let end = start + self.info.visible_row_bytes();

        self.pixels.get(start..end)
    }

    /// Returns the visible pixels of a row, excluding pitch padding.
    pub fn row_mut(&mut self, y: u32) -> Option<&mut [u8]> {
        if y >= self.height() {
            return None;
        }

        let start = y as usize * self.pitch();
        let end = start + self.info.visible_row_bytes();

        self.pixels.get_mut(start..end)
    }
}

fn validate_buffer(
    buffer_len: usize,
    info: SurfaceInfo,
) -> Result<(), SurfaceError> {
    let required = info.required_buffer_size()?;

    if buffer_len < required {
        return Err(SurfaceError::BufferTooSmall {
            available: buffer_len,
            required,
        });
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    EmptySurface,
    SizeOverflow,
    PitchTooSmall {
        pitch: usize,
        minimum: usize,
    },
    BufferTooSmall {
        available: usize,
        required: usize,
    },
    UninitializedFormat,
    NullBuffer,
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySurface => write!(f, "surface dimensions cannot be zero"),
            Self::SizeOverflow => write!(f, "surface size calculation overflowed"),
            Self::PitchTooSmall { pitch, minimum } => {
                write!(f, "surface pitch {pitch} is smaller than {minimum}")
            }
            Self::BufferTooSmall {
                available,
                required,
            } => {
                write!(
                    f,
                    "surface buffer has {available} bytes but requires {required}"
                )
            }
            Self::UninitializedFormat => {
                write!(f, "surface has no initialized pixel format")
            }
            Self::NullBuffer => {
                write!(f, "surface buffer pointer is null")
            }
        }
    }
}