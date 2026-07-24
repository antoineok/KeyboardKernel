pub mod blit;
pub mod compositor;
pub mod framebuffer;
mod smp;
pub mod surface;

pub use blit::{blit_alpha, blit_opaque, BlitError};

pub use surface::{PixelFormat, Surface, SurfaceError, SurfaceInfo, SurfaceMut};

pub use compositor::{
    BlendMode, Compositor, CompositorError, ImageLayer, Layer, Point, Rect, SolidLayer, SourceRect,
};

pub use smp::{
    graphics_core_ready, run_graphics_core, GraphicsCoreProxy, GraphicsRequestResult,
    GraphicsSubmitError,
};
