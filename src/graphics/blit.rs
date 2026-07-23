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
                write!(f, "unsupported opaque blit source format: {format:?}")
            }

            Self::UnsupportedDestinationFormat(format) => {
                write!(f, "unsupported opaque blit destination format: {format:?}")
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

fn source_channel_order(format: PixelFormat) -> Result<ChannelOrder, BlitError> {
    match format {
        PixelFormat::Rgbx8888 | PixelFormat::Rgba8888 => Ok(ChannelOrder::Rgb),

        PixelFormat::Bgrx8888 | PixelFormat::Bgra8888 => Ok(ChannelOrder::Bgr),

        PixelFormat::A8 | PixelFormat::None => Err(BlitError::UnsupportedSourceFormat(format)),
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

/// Copies a rectangular region from `source` into an opaque destination.
///
/// Source alpha is ignored. The destination must be RGBX8888 or BGRX8888.
///
/// Both the source rectangle and destination rectangle are clipped. A
/// completely clipped operation succeeds without modifying the destination.
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

    if width == 0 || height == 0 {
        return Ok(());
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

    // Completely outside the source.
    if source_x >= source_width || source_y >= source_height {
        return Ok(());
    }

    // Clip against the source's right and bottom edges.
    width = width.min(source_width - source_x);
    height = height.min(source_height - source_y);

    // Clip against the destination's left edge.
    if destination_x < 0 {
        let clipped = (-destination_x).min(width);

        source_x += clipped;
        width -= clipped;
        destination_x = 0;
    }

    // Clip against the destination's top edge.
    if destination_y < 0 {
        let clipped = (-destination_y).min(height);

        source_y += clipped;
        height -= clipped;
        destination_y = 0;
    }

    if width <= 0 || height <= 0 {
        return Ok(());
    }

    // Completely outside the destination.
    if destination_x >= destination_width || destination_y >= destination_height {
        return Ok(());
    }

    // Clip against the destination's right and bottom edges.
    width = width.min(destination_width - destination_x);
    height = height.min(destination_height - destination_y);

    if width <= 0 || height <= 0 {
        return Ok(());
    }

    let source_x = usize::try_from(source_x).map_err(|_| BlitError::SizeOverflow)?;
    let source_y = u32::try_from(source_y).map_err(|_| BlitError::SizeOverflow)?;

    let destination_x = usize::try_from(destination_x).map_err(|_| BlitError::SizeOverflow)?;
    let destination_y = u32::try_from(destination_y).map_err(|_| BlitError::SizeOverflow)?;

    let width = usize::try_from(width).map_err(|_| BlitError::SizeOverflow)?;
    let height = u32::try_from(height).map_err(|_| BlitError::SizeOverflow)?;

    let bytes_per_pixel = 4usize;

    let source_byte_x = source_x
        .checked_mul(bytes_per_pixel)
        .ok_or(BlitError::SizeOverflow)?;

    let destination_byte_x = destination_x
        .checked_mul(bytes_per_pixel)
        .ok_or(BlitError::SizeOverflow)?;

    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or(BlitError::SizeOverflow)?;

    for row in 0..height {
        let source_row = source
            .row(source_y + row)
            .expect("clipped source row must exist");

        let destination_row = destination
            .row_mut(destination_y + row)
            .expect("clipped destination row must exist");

        let source_pixels = &source_row[source_byte_x..source_byte_x + row_bytes];

        let destination_pixels =
            &mut destination_row[destination_byte_x..destination_byte_x + row_bytes];

        if source_order == destination_order {
            destination_pixels.copy_from_slice(source_pixels);
            continue;
        }

        for pixel in 0..width {
            let offset = pixel * bytes_per_pixel;

            destination_pixels[offset] = source_pixels[offset + 2];
            destination_pixels[offset + 1] = source_pixels[offset + 1];
            destination_pixels[offset + 2] = source_pixels[offset];

            // The destination is RGBX/BGRX. Keep it canonically opaque even
            // though the display ignores this byte.
            destination_pixels[offset + 3] = 0xff;
        }
    }

    Ok(())
}
