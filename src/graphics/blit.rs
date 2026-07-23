use core::fmt;

use super::surface::{PixelFormat, Surface, SurfaceMut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlitError {
    UnsupportedSourceFormat(PixelFormat),
    UnsupportedDestinationFormat(PixelFormat),
    SizeOverflow,
}

impl fmt::Display for BlitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSourceFormat(format) => {
                write!(f, "unsupported blit source format: {format:?}")
            }

            Self::UnsupportedDestinationFormat(format) => {
                write!(f, "unsupported blit destination format: {format:?}")
            }

            Self::SizeOverflow => {
                write!(f, "blit size calculation overflowed")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChannelOrder {
    Rgb,
    Bgr,
}

impl ChannelOrder {
    const fn offsets(self) -> (usize, usize, usize) {
        match self {
            Self::Rgb => (0, 1, 2),
            Self::Bgr => (2, 1, 0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClippedBlit {
    source_x: usize,
    source_y: u32,

    destination_x: usize,
    destination_y: u32,

    width: usize,
    height: u32,
}

fn source_channel_order(format: PixelFormat) -> Result<ChannelOrder, BlitError> {
    match format {
        PixelFormat::Rgbx8888 | PixelFormat::Rgba8888 => Ok(ChannelOrder::Rgb),

        PixelFormat::Bgrx8888 | PixelFormat::Bgra8888 => Ok(ChannelOrder::Bgr),

        PixelFormat::A8 | PixelFormat::None => Err(BlitError::UnsupportedSourceFormat(format)),
    }
}

fn alpha_source_channel_order(format: PixelFormat) -> Result<ChannelOrder, BlitError> {
    match format {
        PixelFormat::Rgba8888 => Ok(ChannelOrder::Rgb),
        PixelFormat::Bgra8888 => Ok(ChannelOrder::Bgr),

        PixelFormat::Rgbx8888 | PixelFormat::Bgrx8888 | PixelFormat::A8 | PixelFormat::None => {
            Err(BlitError::UnsupportedSourceFormat(format))
        }
    }
}

fn destination_channel_order(format: PixelFormat) -> Result<ChannelOrder, BlitError> {
    match format {
        PixelFormat::Rgbx8888 => Ok(ChannelOrder::Rgb),
        PixelFormat::Bgrx8888 => Ok(ChannelOrder::Bgr),

        PixelFormat::Rgba8888 | PixelFormat::Bgra8888 | PixelFormat::A8 | PixelFormat::None => {
            Err(BlitError::UnsupportedDestinationFormat(format))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clip_blit(
    source: &Surface<'_>,
    destination: &SurfaceMut<'_>,
    source_x: u32,
    source_y: u32,
    destination_x: i64,
    destination_y: i64,
    width: u32,
    height: u32,
) -> Result<Option<ClippedBlit>, BlitError> {
    if width == 0 || height == 0 {
        return Ok(None);
    }

    let source_width = i64::from(source.width());
    let source_height = i64::from(source.height());

    let destination_width = i64::from(destination.width());
    let destination_height = i64::from(destination.height());

    let mut source_x = i64::from(source_x);
    let mut source_y = i64::from(source_y);

    let mut destination_x = destination_x;
    let mut destination_y = destination_y;

    let mut width = i64::from(width);
    let mut height = i64::from(height);

    // The requested source origin is already outside its surface.
    if source_x >= source_width || source_y >= source_height {
        return Ok(None);
    }

    // Clip against the source's right and bottom edges.
    width = width.min(source_width - source_x);
    height = height.min(source_height - source_y);

    // Clip against the destination's left edge.
    if destination_x < 0 {
        let clipped = destination_x.saturating_neg().min(width);

        source_x += clipped;
        width -= clipped;
        destination_x = 0;
    }

    // Clip against the destination's top edge.
    if destination_y < 0 {
        let clipped = destination_y.saturating_neg().min(height);

        source_y += clipped;
        height -= clipped;
        destination_y = 0;
    }

    if width <= 0 || height <= 0 {
        return Ok(None);
    }

    // The destination origin is entirely outside the surface.
    if destination_x >= destination_width || destination_y >= destination_height {
        return Ok(None);
    }

    // Clip against the destination's right and bottom edges.
    width = width.min(destination_width - destination_x);
    height = height.min(destination_height - destination_y);

    if width <= 0 || height <= 0 {
        return Ok(None);
    }

    Ok(Some(ClippedBlit {
        source_x: usize::try_from(source_x).map_err(|_| BlitError::SizeOverflow)?,

        source_y: u32::try_from(source_y).map_err(|_| BlitError::SizeOverflow)?,

        destination_x: usize::try_from(destination_x).map_err(|_| BlitError::SizeOverflow)?,

        destination_y: u32::try_from(destination_y).map_err(|_| BlitError::SizeOverflow)?,

        width: usize::try_from(width).map_err(|_| BlitError::SizeOverflow)?,

        height: u32::try_from(height).map_err(|_| BlitError::SizeOverflow)?,
    }))
}

/// Copies pixels into an opaque RGBX/BGRX destination.
///
/// Source alpha, when present, is ignored.
#[allow(clippy::too_many_arguments)]
pub fn blit_opaque(
    source: &Surface<'_>,
    destination: &mut SurfaceMut<'_>,
    source_x: u32,
    source_y: u32,
    destination_x: i64,
    destination_y: i64,
    width: u32,
    height: u32,
) -> Result<(), BlitError> {
    let source_order = source_channel_order(source.format())?;
    let destination_order = destination_channel_order(destination.format())?;

    let Some(region) = clip_blit(
        source,
        destination,
        source_x,
        source_y,
        destination_x,
        destination_y,
        width,
        height,
    )?
    else {
        return Ok(());
    };

    let source_byte_x = region
        .source_x
        .checked_mul(4)
        .ok_or(BlitError::SizeOverflow)?;

    let destination_byte_x = region
        .destination_x
        .checked_mul(4)
        .ok_or(BlitError::SizeOverflow)?;

    let row_bytes = region.width.checked_mul(4).ok_or(BlitError::SizeOverflow)?;

    let source_is_opaque = matches!(
        source.format(),
        PixelFormat::Rgbx8888 | PixelFormat::Bgrx8888
    );

    for row in 0..region.height {
        let source_row = source
            .row(region.source_y + row)
            .expect("clipped source row must exist");

        let destination_row = destination
            .row_mut(region.destination_y + row)
            .expect("clipped destination row must exist");

        let source_pixels = &source_row[source_byte_x..source_byte_x + row_bytes];

        let destination_pixels =
            &mut destination_row[destination_byte_x..destination_byte_x + row_bytes];

        if source_order == destination_order && source_is_opaque {
            destination_pixels.copy_from_slice(source_pixels);
            continue;
        }

        let (source_r, source_g, source_b) = source_order.offsets();
        let (destination_r, destination_g, destination_b) = destination_order.offsets();

        for pixel in 0..region.width {
            let offset = pixel * 4;

            destination_pixels[offset + destination_r] = source_pixels[offset + source_r];

            destination_pixels[offset + destination_g] = source_pixels[offset + source_g];

            destination_pixels[offset + destination_b] = source_pixels[offset + source_b];

            destination_pixels[offset + 3] = 0xff;
        }
    }

    Ok(())
}

/// Alpha-blends an RGBA8888/BGRA8888 source over an opaque RGBX/BGRX
/// destination.
///
/// Source pixels use straight, non-premultiplied alpha.
#[allow(clippy::too_many_arguments)]
pub fn blit_alpha(
    source: &Surface<'_>,
    destination: &mut SurfaceMut<'_>,
    source_x: u32,
    source_y: u32,
    destination_x: i64,
    destination_y: i64,
    width: u32,
    height: u32,
) -> Result<(), BlitError> {
    let source_order = alpha_source_channel_order(source.format())?;
    let destination_order = destination_channel_order(destination.format())?;

    let Some(region) = clip_blit(
        source,
        destination,
        source_x,
        source_y,
        destination_x,
        destination_y,
        width,
        height,
    )?
    else {
        return Ok(());
    };

    let source_byte_x = region
        .source_x
        .checked_mul(4)
        .ok_or(BlitError::SizeOverflow)?;

    let destination_byte_x = region
        .destination_x
        .checked_mul(4)
        .ok_or(BlitError::SizeOverflow)?;

    let row_bytes = region.width.checked_mul(4).ok_or(BlitError::SizeOverflow)?;

    let (source_r, source_g, source_b) = source_order.offsets();
    let (destination_r, destination_g, destination_b) = destination_order.offsets();

    for row in 0..region.height {
        let source_row = source
            .row(region.source_y + row)
            .expect("clipped source row must exist");

        let destination_row = destination
            .row_mut(region.destination_y + row)
            .expect("clipped destination row must exist");

        let source_pixels = &source_row[source_byte_x..source_byte_x + row_bytes];

        let destination_pixels =
            &mut destination_row[destination_byte_x..destination_byte_x + row_bytes];

        for pixel in 0..region.width {
            let offset = pixel * 4;
            let alpha = source_pixels[offset + 3];

            match alpha {
                0 => {
                    // Completely transparent: keep the destination untouched.
                }

                255 => {
                    // Completely opaque: avoid unnecessary multiplications.
                    destination_pixels[offset + destination_r] = source_pixels[offset + source_r];

                    destination_pixels[offset + destination_g] = source_pixels[offset + source_g];

                    destination_pixels[offset + destination_b] = source_pixels[offset + source_b];
                }

                _ => {
                    destination_pixels[offset + destination_r] = blend_channel(
                        source_pixels[offset + source_r],
                        destination_pixels[offset + destination_r],
                        alpha,
                    );

                    destination_pixels[offset + destination_g] = blend_channel(
                        source_pixels[offset + source_g],
                        destination_pixels[offset + destination_g],
                        alpha,
                    );

                    destination_pixels[offset + destination_b] = blend_channel(
                        source_pixels[offset + source_b],
                        destination_pixels[offset + destination_b],
                        alpha,
                    );
                }
            }

            // The destination remains opaque.
            destination_pixels[offset + 3] = 0xff;
        }
    }

    Ok(())
}

#[inline(always)]
fn blend_channel(source: u8, destination: u8, alpha: u8) -> u8 {
    let alpha = u32::from(alpha);
    let inverse_alpha = 255 - alpha;

    let blended = u32::from(source) * alpha + u32::from(destination) * inverse_alpha;

    // Adding 127 gives normal rounding instead of always rounding down.
    ((blended + 127) / 255) as u8
}
