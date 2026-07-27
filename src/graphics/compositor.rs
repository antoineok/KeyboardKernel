use core::fmt;

use super::{
    blit::{blit_alpha, blit_opaque, BlitError},
    surface::{PixelFormat, Surface, SurfaceMut},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i64, y: i64, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SourceRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn whole(surface: &Surface<'_>) -> Self {
        Self {
            x: 0,
            y: 0,
            width: surface.width(),
            height: surface.height(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// Copy source RGB values and ignore source alpha.
    Opaque,

    /// Blend RGBA/BGRA source pixels using straight alpha.
    Alpha,
}

#[derive(Clone, Copy)]
pub struct ImageLayer<'a> {
    pub source: Surface<'a>,
    pub source_rect: SourceRect,
    pub position: Point,
    pub blend_mode: BlendMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolidLayer {
    pub bounds: Rect,

    /// Colour in `0xRRGGBBAA` format.
    pub color: u32,
}

#[derive(Clone, Copy)]
pub enum Layer<'a> {
    Solid(SolidLayer),
    Image(ImageLayer<'a>),
}

pub struct Compositor {
    /// Full-frame clear colour in `0xRRGGBBAA` format.
    background_color: u32,
}

impl Compositor {
    pub const fn new(background_color: u32) -> Self {
        Self { background_color }
    }

    pub const fn background_color(&self) -> u32 {
        self.background_color
    }

    pub fn set_background_color(&mut self, color: u32) {
        self.background_color = color;
    }

    pub fn clear(&self, destination: &mut SurfaceMut<'_>) -> Result<(), CompositorError> {
        fill_rect(
            destination,
            Rect::new(0, 0, destination.width(), destination.height()),
            self.background_color,
        )
    }

    pub fn render_layer(
        &self,
        destination: &mut SurfaceMut<'_>,
        layer: Layer<'_>,
    ) -> Result<(), CompositorError> {
        match layer {
            Layer::Solid(layer) => fill_rect(destination, layer.bounds, layer.color),
            Layer::Image(layer) => {
                let source = layer.source_rect;
                match layer.blend_mode {
                    BlendMode::Opaque => blit_opaque(
                        &layer.source,
                        destination,
                        source.x,
                        source.y,
                        layer.position.x,
                        layer.position.y,
                        source.width,
                        source.height,
                    )
                    .map_err(Into::into),
                    BlendMode::Alpha => blit_alpha(
                        &layer.source,
                        destination,
                        source.x,
                        source.y,
                        layer.position.x,
                        layer.position.y,
                        source.width,
                        source.height,
                    )
                    .map_err(Into::into),
                }
            }
        }
    }

    /// Render a complete frame. Layers are drawn back-to-front.
    pub fn render(
        &self,
        destination: &mut SurfaceMut<'_>,
        layers: &[Layer<'_>],
    ) -> Result<(), CompositorError> {
        self.clear(destination)?;
        for &layer in layers {
            self.render_layer(destination, layer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorError {
    Blit(BlitError),
    UnsupportedDestinationFormat(PixelFormat),
}

impl From<BlitError> for CompositorError {
    fn from(error: BlitError) -> Self {
        Self::Blit(error)
    }
}

impl fmt::Display for CompositorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blit(error) => write!(f, "layer blit failed: {error}"),

            Self::UnsupportedDestinationFormat(format) => {
                write!(f, "unsupported compositor destination format: {format:?}")
            }
        }
    }
}

fn destination_offsets(format: PixelFormat) -> Result<(usize, usize, usize), CompositorError> {
    match format {
        PixelFormat::Rgbx8888 => Ok((0, 1, 2)),
        PixelFormat::Bgrx8888 => Ok((2, 1, 0)),

        PixelFormat::Rgba8888 | PixelFormat::Bgra8888 | PixelFormat::A8 | PixelFormat::None => {
            Err(CompositorError::UnsupportedDestinationFormat(format))
        }
    }
}

fn fill_rect(
    destination: &mut SurfaceMut<'_>,
    bounds: Rect,
    color: u32,
) -> Result<(), CompositorError> {
    if bounds.width == 0 || bounds.height == 0 {
        return Ok(());
    }

    let destination_width = i64::from(destination.width());
    let destination_height = i64::from(destination.height());

    let right = bounds.x.saturating_add(i64::from(bounds.width));

    let bottom = bounds.y.saturating_add(i64::from(bounds.height));

    let left = bounds.x.max(0);
    let top = bounds.y.max(0);
    let right = right.min(destination_width);
    let bottom = bottom.min(destination_height);

    if left >= right || top >= bottom {
        return Ok(());
    }

    let left = left as usize;
    let top = top as u32;
    let right = right as usize;
    let bottom = bottom as u32;

    let [red, green, blue, alpha] = color.to_be_bytes();

    let (red_offset, green_offset, blue_offset) = destination_offsets(destination.format())?;

    for y in top..bottom {
        let row = destination
            .row_mut(y)
            .expect("clipped compositor row must exist");

        for x in left..right {
            let offset = x * 4;

            match alpha {
                0 => {}

                255 => {
                    row[offset + red_offset] = red;
                    row[offset + green_offset] = green;
                    row[offset + blue_offset] = blue;
                }

                _ => {
                    row[offset + red_offset] = blend_channel(red, row[offset + red_offset], alpha);

                    row[offset + green_offset] =
                        blend_channel(green, row[offset + green_offset], alpha);

                    row[offset + blue_offset] =
                        blend_channel(blue, row[offset + blue_offset], alpha);
                }
            }

            // RGBX/BGRX destination remains canonically opaque.
            row[offset + 3] = 0xff;
        }
    }

    Ok(())
}

#[inline(always)]
fn blend_channel(source: u8, destination: u8, alpha: u8) -> u8 {
    let alpha = u32::from(alpha);
    let inverse_alpha = 255 - alpha;

    let blended = u32::from(source) * alpha + u32::from(destination) * inverse_alpha;

    ((blended + 127) / 255) as u8
}
