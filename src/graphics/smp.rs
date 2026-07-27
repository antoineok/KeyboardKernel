use core::{
    slice,
    sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
};

use crate::arch::cpu::{current_id, send_event, wait_for_event, SpscQueue};

use super::{
    framebuffer::FRAMEBUFFER, BlendMode, Compositor, ImageLayer, Layer, Point, SourceRect, Surface,
    SurfaceInfo,
};

const GRAPHICS_QUEUE_SLOTS: usize = 5;

const RESULT_NONE: u8 = 0;
const RESULT_OK: u8 = 1;
const RESULT_INVALID_SOURCE: u8 = 2;
const RESULT_FRAMEBUFFER_UNAVAILABLE: u8 = 3;
const RESULT_COMPOSITOR_FAILED: u8 = 4;
const RESULT_PRESENT_FAILED: u8 = 5;

#[derive(Clone, Copy)]
struct SharedSurface {
    pixels: usize,
    length: usize,
    info: SurfaceInfo,
}

impl SharedSurface {
    fn from_surface(surface: &Surface<'_>) -> Self {
        Self {
            pixels: surface.pixels().as_ptr() as usize,
            length: surface.pixels().len(),
            info: surface.info(),
        }
    }

    /// Reconstruct an immutable surface shared by CPU0.
    ///
    /// # Safety
    ///
    /// The producer must keep the source allocation alive and immutable until
    /// this command has completed. `GraphicsCoreProxy::submit_image` exposes
    /// the raw request ID so CPU0 can preserve that lifetime until completion.
    unsafe fn as_surface<'a>(self) -> Result<Surface<'a>, super::SurfaceError> {
        let pixels = unsafe { slice::from_raw_parts(self.pixels as *const u8, self.length) };
        Surface::new(pixels, self.info)
    }
}

#[derive(Clone, Copy)]
struct RenderImageCommand {
    request_id: u64,
    source: SharedSurface,
    source_rect: SourceRect,
    position: Point,
    blend_mode: BlendMode,
    clear_color: u32,
}

static CPU0_TO_CPU1_GRAPHICS: SpscQueue<RenderImageCommand, GRAPHICS_QUEUE_SLOTS> =
    SpscQueue::new();

static GRAPHICS_CORE_READY: AtomicBool = AtomicBool::new(false);
static GRAPHICS_COMPLETED_REQUEST: AtomicU64 = AtomicU64::new(0);
static GRAPHICS_COMPLETION_RESULT: AtomicU8 = AtomicU8::new(RESULT_NONE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsSubmitError {
    WorkerNotReady,
    QueueFull,
    RequestPending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsRequestResult {
    Complete,
    InvalidSource,
    FramebufferUnavailable,
    CompositorFailed,
    PresentFailed,
}

/// CPU0-side command producer for the graphics core.
///
/// CPU1 is the sole runtime owner of the framebuffer and compositor. CPU0 may
/// prepare/load immutable assets, then submit descriptors through this proxy.
pub struct GraphicsCoreProxy {
    next_request_id: u64,
    pending_request: Option<u64>,
}

impl GraphicsCoreProxy {
    pub const fn new() -> Self {
        Self {
            next_request_id: 1,
            pending_request: None,
        }
    }

    /// Submit one complete-frame image render to CPU1.
    ///
    /// The source pixels are borrowed only conceptually: the queue stores a raw
    /// immutable descriptor so it can cross cores. The caller must keep
    /// `source` alive and unchanged until [`Self::request_result`] reports that
    /// this request has completed.
    pub fn submit_image(
        &mut self,
        source: &Surface<'_>,
        source_rect: SourceRect,
        position: Point,
        blend_mode: BlendMode,
        clear_color: u32,
    ) -> Result<u64, GraphicsSubmitError> {
        assert_eq!(
            current_id().raw(),
            0,
            "graphics commands must currently be submitted by CPU0"
        );

        if !graphics_core_ready() {
            return Err(GraphicsSubmitError::WorkerNotReady);
        }

        if self.pending_request.is_some() {
            return Err(GraphicsSubmitError::RequestPending);
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }

        let command = RenderImageCommand {
            request_id,
            source: SharedSurface::from_surface(source),
            source_rect,
            position,
            blend_mode,
            clear_color,
        };

        CPU0_TO_CPU1_GRAPHICS
            .try_push(command)
            .map_err(|_| GraphicsSubmitError::QueueFull)?;
        self.pending_request = Some(request_id);

        send_event();

        Ok(request_id)
    }

    pub fn request_result(&mut self, request_id: u64) -> Option<GraphicsRequestResult> {
        if self.pending_request != Some(request_id) {
            return None;
        }

        if GRAPHICS_COMPLETED_REQUEST.load(Ordering::Acquire) != request_id {
            return None;
        }

        let result = match GRAPHICS_COMPLETION_RESULT.load(Ordering::Acquire) {
            RESULT_OK => GraphicsRequestResult::Complete,
            RESULT_INVALID_SOURCE => GraphicsRequestResult::InvalidSource,
            RESULT_FRAMEBUFFER_UNAVAILABLE => GraphicsRequestResult::FramebufferUnavailable,
            RESULT_COMPOSITOR_FAILED => GraphicsRequestResult::CompositorFailed,
            RESULT_PRESENT_FAILED => GraphicsRequestResult::PresentFailed,
            _ => return None,
        };

        self.pending_request = None;
        Some(result)
    }
}

impl Default for GraphicsCoreProxy {
    fn default() -> Self {
        Self::new()
    }
}

pub fn graphics_core_ready() -> bool {
    GRAPHICS_CORE_READY.load(Ordering::Acquire)
}

/// Permanent CPU1 graphics worker.
///
/// CPU0 initializes the mailbox framebuffer before releasing secondary CPUs.
/// From this function onward CPU1 is the sole runtime owner of `FRAMEBUFFER`
/// and the mailbox calls used to present framebuffer pages.
pub fn run_graphics_core() -> ! {
    let cpu = current_id();
    assert_eq!(cpu.raw(), 1, "graphics worker must run on CPU1");

    let framebuffer = FRAMEBUFFER.get();
    let mut compositor = Compositor::new(0x000000ff);

    // Validate that CPU0 completed framebuffer initialization before handing
    // ownership to this core.
    framebuffer
        .as_surface_mut()
        .expect("framebuffer unavailable when CPU1 claimed graphics ownership");

    GRAPHICS_CORE_READY.store(true, Ordering::Release);
    send_event();
    crate::info!("{} graphics worker ready", cpu);

    loop {
        let Some(command) = CPU0_TO_CPU1_GRAPHICS.try_pop() else {
            wait_for_event();
            continue;
        };

        compositor.set_background_color(command.clear_color);

        let result = render_image_command(framebuffer, &compositor, command);
        GRAPHICS_COMPLETION_RESULT.store(result, Ordering::Release);
        GRAPHICS_COMPLETED_REQUEST.store(command.request_id, Ordering::Release);

        send_event();

        if result == RESULT_OK {
            crate::info!("{} presented graphics request {}", cpu, command.request_id);
        }
    }
}

fn render_image_command(
    framebuffer: &mut super::framebuffer::Framebuffer,
    compositor: &Compositor,
    command: RenderImageCommand,
) -> u8 {
    let source = match unsafe { command.source.as_surface() } {
        Ok(surface) => surface,
        Err(error) => {
            crate::error!("CPU1 rejected shared graphics surface: {}", error);
            return RESULT_INVALID_SOURCE;
        }
    };

    let layers = [Layer::Image(ImageLayer {
        source,
        source_rect: command.source_rect,
        position: command.position,
        blend_mode: command.blend_mode,
    })];

    {
        let mut backbuffer = match framebuffer.as_surface_mut() {
            Ok(surface) => surface,
            Err(error) => {
                crate::error!("CPU1 framebuffer backbuffer unavailable: {}", error);
                return RESULT_FRAMEBUFFER_UNAVAILABLE;
            }
        };

        if let Err(error) = compositor.render(&mut backbuffer, &layers) {
            crate::error!("CPU1 frame composition failed: {}", error);
            return RESULT_COMPOSITOR_FAILED;
        }
    }

    if !framebuffer.present() {
        crate::error!("CPU1 framebuffer present failed");
        return RESULT_PRESENT_FAILED;
    }

    RESULT_OK
}
