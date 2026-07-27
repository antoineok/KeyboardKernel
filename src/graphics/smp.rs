use alloc::vec::Vec;
use core::{
    slice,
    time::Duration,
    sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
};

use crate::{
    arch::cpu::{current_id, send_event, wait_for_event, SpscQueue},
    storage::fs::vfs::FsId,
    time::time_manager,
    video::player::VideoPlayer,
};

use super::{
    framebuffer::FRAMEBUFFER, BlendMode, Compositor, ImageLayer, Layer, Point, SourceRect, Surface,
    SurfaceInfo,
};

const GRAPHICS_QUEUE_SLOTS: usize = 9;
const OVERLAY_SLOTS: usize = 8;
const VSYNC_CALIBRATION_INTERVALS: u32 = 6;
const FALLBACK_VSYNC_PERIOD: Duration = Duration::from_micros(16_667);

const RESULT_NONE: u8 = 0;
const RESULT_OK: u8 = 1;
const RESULT_INVALID_SOURCE: u8 = 2;
const RESULT_FRAMEBUFFER_UNAVAILABLE: u8 = 3;
const RESULT_COMPOSITOR_FAILED: u8 = 4;
const RESULT_PRESENT_FAILED: u8 = 5;
const RESULT_VIDEO_FAILED: u8 = 6;
const RESULT_OVERLAY_SLOT_INVALID: u8 = 7;

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

#[derive(Clone, Copy)]
struct StartVideoCommand {
    request_id: u64,
    fs: FsId,
    path: &'static str,
}

#[derive(Clone, Copy)]
struct SetOverlayCommand {
    request_id: u64,
    slot: u8,
    source: SharedSurface,
    source_rect: SourceRect,
    position: Point,
    blend_mode: BlendMode,
}

#[derive(Clone, Copy)]
enum GraphicsCommand {
    RenderImage(RenderImageCommand),
    StartVideo(StartVideoCommand),
    StopVideo { request_id: u64 },
    SetOverlay(SetOverlayCommand),
    ClearOverlay { request_id: u64, slot: u8 },
}

impl GraphicsCommand {
    const fn request_id(self) -> u64 {
        match self {
            Self::RenderImage(command) => command.request_id,
            Self::StartVideo(command) => command.request_id,
            Self::StopVideo { request_id } | Self::ClearOverlay { request_id, .. } => request_id,
            Self::SetOverlay(command) => command.request_id,
        }
    }
}

struct OwnedOverlay {
    pixels: Vec<u8>,
    info: SurfaceInfo,
    source_rect: SourceRect,
    position: Point,
    blend_mode: BlendMode,
}

impl OwnedOverlay {
    fn from_command(command: SetOverlayCommand) -> Result<Self, super::SurfaceError> {
        let source = unsafe { command.source.as_surface()? };
        Ok(Self {
            pixels: source.pixels().to_vec(),
            info: source.info(),
            source_rect: command.source_rect,
            position: command.position,
            blend_mode: command.blend_mode,
        })
    }

    fn surface(&self) -> Result<Surface<'_>, super::SurfaceError> {
        Surface::new(&self.pixels, self.info)
    }
}

static CPU0_TO_CPU1_GRAPHICS: SpscQueue<GraphicsCommand, GRAPHICS_QUEUE_SLOTS> = SpscQueue::new();
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
    VideoFailed,
    OverlaySlotInvalid,
}

/// CPU0-side command producer for the graphics core.
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

    fn next_id(&mut self) -> Result<u64, GraphicsSubmitError> {
        assert_eq!(current_id().raw(), 0, "graphics commands must be submitted by CPU0");
        if !graphics_core_ready() {
            return Err(GraphicsSubmitError::WorkerNotReady);
        }
        if self.pending_request.is_some() {
            return Err(GraphicsSubmitError::RequestPending);
        }
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }
        Ok(id)
    }

    fn submit(&mut self, command: GraphicsCommand) -> Result<u64, GraphicsSubmitError> {
        let request_id = command.request_id();
        CPU0_TO_CPU1_GRAPHICS
            .try_push(command)
            .map_err(|_| GraphicsSubmitError::QueueFull)?;
        self.pending_request = Some(request_id);
        send_event();
        Ok(request_id)
    }

    pub fn submit_image(
        &mut self,
        source: &Surface<'_>,
        source_rect: SourceRect,
        position: Point,
        blend_mode: BlendMode,
        clear_color: u32,
    ) -> Result<u64, GraphicsSubmitError> {
        let request_id = self.next_id()?;
        self.submit(GraphicsCommand::RenderImage(RenderImageCommand {
            request_id,
            source: SharedSurface::from_surface(source),
            source_rect,
            position,
            blend_mode,
            clear_color,
        }))
    }

    /// Ask CPU1 to own and continuously play a `kwv1` background.
    pub fn start_video(
        &mut self,
        fs: FsId,
        path: &'static str,
    ) -> Result<u64, GraphicsSubmitError> {
        let request_id = self.next_id()?;
        self.submit(GraphicsCommand::StartVideo(StartVideoCommand {
            request_id,
            fs,
            path,
        }))
    }

    pub fn stop_video(&mut self) -> Result<u64, GraphicsSubmitError> {
        let request_id = self.next_id()?;
        self.submit(GraphicsCommand::StopVideo { request_id })
    }

    /// Copy one persistent image layer into CPU1-owned memory.
    ///
    /// Once the request completes, CPU0 may release or mutate the source.
    pub fn set_overlay_image(
        &mut self,
        slot: usize,
        source: &Surface<'_>,
        source_rect: SourceRect,
        position: Point,
        blend_mode: BlendMode,
    ) -> Result<u64, GraphicsSubmitError> {
        let request_id = self.next_id()?;
        self.submit(GraphicsCommand::SetOverlay(SetOverlayCommand {
            request_id,
            slot: u8::try_from(slot).unwrap_or(u8::MAX),
            source: SharedSurface::from_surface(source),
            source_rect,
            position,
            blend_mode,
        }))
    }

    pub fn clear_overlay(&mut self, slot: usize) -> Result<u64, GraphicsSubmitError> {
        let request_id = self.next_id()?;
        self.submit(GraphicsCommand::ClearOverlay {
            request_id,
            slot: u8::try_from(slot).unwrap_or(u8::MAX),
        })
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
            RESULT_VIDEO_FAILED => GraphicsRequestResult::VideoFailed,
            RESULT_OVERLAY_SLOT_INVALID => GraphicsRequestResult::OverlaySlotInvalid,
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

/// Permanent CPU1 graphics scheduler.
///
/// CPU1 owns the framebuffer, the compositor, KWVIDEO playback and RPIVID.
/// Video is just the back-most scene layer; persistent overlays are drawn over
/// the current video frame whenever either the video or an overlay changes.
pub fn run_graphics_core() -> ! {
    let cpu = current_id();
    assert_eq!(cpu.raw(), 1, "graphics worker must run on CPU1");

    let framebuffer = FRAMEBUFFER.get();
    let mut compositor = Compositor::new(0x000000ff);
    let mut video: Option<VideoPlayer> = None;
    let mut overlays: [Option<OwnedOverlay>; OVERLAY_SLOTS] = core::array::from_fn(|_| None);
    let mut scene_dirty = false;
    let mut backbuffer_ready = false;

    framebuffer
        .as_surface_mut()
        .expect("framebuffer unavailable when CPU1 claimed graphics ownership");

    GRAPHICS_CORE_READY.store(true, Ordering::Release);
    send_event();
    crate::info!("{} graphics/video scheduler ready", cpu);

    let vsync_period = calibrate_vsync_period(framebuffer).unwrap_or(FALLBACK_VSYNC_PERIOD);
    crate::info!(
        "CPU1 display VSYNC period: {} us (~{} Hz)",
        vsync_period.as_micros(),
        if vsync_period.as_micros() == 0 {
            0
        } else {
            1_000_000u128 / vsync_period.as_micros()
        },
    );

    loop {
        let mut handled_command = false;
        while let Some(command) = CPU0_TO_CPU1_GRAPHICS.try_pop() {
            handled_command = true;
            let request_id = command.request_id();
            let result = match command {
                GraphicsCommand::RenderImage(command) => {
                    compositor.set_background_color(command.clear_color);
                    render_image_command(framebuffer, &compositor, command)
                }
                GraphicsCommand::StartVideo(command) => match VideoPlayer::open(command.fs, command.path) {
                    Ok(player) => {
                        crate::info!("CPU1 started KWVIDEO background: {}", command.path);
                        video = Some(player);
                        scene_dirty = true;
                        backbuffer_ready = false;
                        RESULT_OK
                    }
                    Err(error) => {
                        crate::error!("CPU1 failed to start KWVIDEO {}: {}", command.path, error);
                        RESULT_VIDEO_FAILED
                    }
                },
                GraphicsCommand::StopVideo { .. } => {
                    video = None;
                    scene_dirty = true;
                    backbuffer_ready = false;
                    RESULT_OK
                }
                GraphicsCommand::SetOverlay(command) => {
                    let slot = usize::from(command.slot);
                    if slot >= overlays.len() {
                        RESULT_OVERLAY_SLOT_INVALID
                    } else {
                        match OwnedOverlay::from_command(command) {
                            Ok(overlay) => {
                                overlays[slot] = Some(overlay);
                                scene_dirty = true;
                                backbuffer_ready = false;
                                RESULT_OK
                            }
                            Err(error) => {
                                crate::error!("CPU1 rejected shared overlay surface: {}", error);
                                RESULT_INVALID_SOURCE
                            }
                        }
                    }
                }
                GraphicsCommand::ClearOverlay { slot, .. } => {
                    let slot = usize::from(slot);
                    if slot >= overlays.len() {
                        RESULT_OVERLAY_SLOT_INVALID
                    } else {
                        overlays[slot] = None;
                        scene_dirty = true;
                        backbuffer_ready = false;
                        RESULT_OK
                    }
                }
            };

            GRAPHICS_COMPLETION_RESULT.store(result, Ordering::Release);
            GRAPHICS_COMPLETED_REQUEST.store(request_id, Ordering::Release);
            send_event();
        }

        // Stage the next decoded video frame as soon as the previous page has
        // been presented. Composition then happens into the hidden page long
        // before the next media deadline, leaving the deadline path with only
        // VSYNC + page flip.
        if let Some(player) = video.as_mut() {
            match player.stage_next_frame() {
                Ok(true) => {
                    scene_dirty = true;
                    backbuffer_ready = false;
                },
                Ok(false) => {}
                Err(error) => {
                    crate::error!("CPU1 KWVIDEO playback failed: {}", error);
                    video = None;
                    scene_dirty = true;
                }
            }
        }

        if scene_dirty {
            let result = compose_scene(framebuffer, &compositor, video.as_ref(), &overlays);
            if result != RESULT_OK {
                crate::error!("CPU1 scene composition failed with result {}", result);
                backbuffer_ready = false;
            } else {
                backbuffer_ready = true;
            }
            scene_dirty = false;
        }

        if let Some(player) = video.as_mut() {
            if backbuffer_ready
                && player.presentation_window_open(time_manager().uptime(), vsync_period)
            {
                let result = present_scene(framebuffer);
                if result != RESULT_OK {
                    crate::error!("CPU1 scene presentation failed with result {}", result);
                    backbuffer_ready = false;
                    scene_dirty = true;
                } else {
                    player.frame_presented(time_manager().uptime());
                    backbuffer_ready = false;
                }
            }
        } else if backbuffer_ready {
            // Static scene changes do not have a media clock. Present them at
            // the next vblank as soon as composition completes.
            let result = present_scene(framebuffer);
            if result != RESULT_OK {
                crate::error!("CPU1 static scene presentation failed with result {}", result);
            }
            backbuffer_ready = false;
        }

        if let Some(player) = video.as_mut() {
            if let Err(error) = player.prepare_if_needed(time_manager().uptime(), vsync_period) {
                crate::error!("CPU1 KWVIDEO decode preparation failed: {}", error);
                video = None;
                scene_dirty = true;
            }
            // A running video has media deadlines but no timer IRQ yet. CPU1 is
            // dedicated to visuals, so poll the scheduler clock between frames.
            core::hint::spin_loop();
        } else if !handled_command {
            wait_for_event();
        }
    }
}

fn calibrate_vsync_period(framebuffer: &mut super::framebuffer::Framebuffer) -> Option<Duration> {
    if !framebuffer.wait_for_vsync() {
        return None;
    }
    let started = time_manager().uptime();
    for _ in 0..VSYNC_CALIBRATION_INTERVALS {
        if !framebuffer.wait_for_vsync() {
            return None;
        }
    }
    let elapsed = time_manager().uptime().saturating_sub(started);
    let nanos = elapsed.as_nanos() / u128::from(VSYNC_CALIBRATION_INTERVALS);
    if nanos == 0 || nanos > u128::from(u64::MAX) {
        return None;
    }
    Some(Duration::from_nanos(nanos as u64))
}

fn compose_scene(
    framebuffer: &mut super::framebuffer::Framebuffer,
    compositor: &Compositor,
    video: Option<&VideoPlayer>,
    overlays: &[Option<OwnedOverlay>; OVERLAY_SLOTS],
) -> u8 {
    {
        let mut backbuffer = match framebuffer.as_surface_mut() {
            Ok(surface) => surface,
            Err(error) => {
                crate::error!("CPU1 framebuffer backbuffer unavailable: {}", error);
                return RESULT_FRAMEBUFFER_UNAVAILABLE;
            }
        };

        let video_frame = video.and_then(VideoPlayer::current_frame);
        if video_frame.is_none() {
            if let Err(error) = compositor.clear(&mut backbuffer) {
                crate::error!("CPU1 framebuffer clear failed: {}", error);
                return RESULT_COMPOSITOR_FAILED;
            }
        }

        if let Some(frame) = video_frame {
            assert_eq!(frame.width(), backbuffer.width(), "KWVIDEO background width must match framebuffer");
            assert_eq!(frame.height(), backbuffer.height(), "KWVIDEO background height must match framebuffer");
            let info = match SurfaceInfo::new(
                frame.width(),
                frame.height(),
                frame.pitch(),
                super::PixelFormat::Rgbx8888,
            ) {
                Ok(info) => info,
                Err(error) => {
                    crate::error!("CPU1 rejected decoded video surface: {}", error);
                    return RESULT_INVALID_SOURCE;
                }
            };
            let surface = match Surface::new(frame.pixels(), info) {
                Ok(surface) => surface,
                Err(error) => {
                    crate::error!("CPU1 rejected decoded video pixels: {}", error);
                    return RESULT_INVALID_SOURCE;
                }
            };
            if let Err(error) = compositor.render_layer(
                &mut backbuffer,
                Layer::Image(ImageLayer {
                    source: surface,
                    source_rect: SourceRect::whole(&surface),
                    position: Point::new(0, 0),
                    blend_mode: BlendMode::Opaque,
                }),
            ) {
                crate::error!("CPU1 video background composition failed: {}", error);
                return RESULT_COMPOSITOR_FAILED;
            }
        }

        for overlay in overlays.iter().flatten() {
            let source = match overlay.surface() {
                Ok(surface) => surface,
                Err(error) => {
                    crate::error!("CPU1 stored overlay became invalid: {}", error);
                    return RESULT_INVALID_SOURCE;
                }
            };
            if let Err(error) = compositor.render_layer(
                &mut backbuffer,
                Layer::Image(ImageLayer {
                    source,
                    source_rect: overlay.source_rect,
                    position: overlay.position,
                    blend_mode: overlay.blend_mode,
                }),
            ) {
                crate::error!("CPU1 overlay composition failed: {}", error);
                return RESULT_COMPOSITOR_FAILED;
            }
        }
    }

    RESULT_OK
}

fn present_scene(framebuffer: &mut super::framebuffer::Framebuffer) -> u8 {
    if !framebuffer.wait_for_vsync() {
        crate::error!("CPU1 framebuffer VSYNC wait failed");
        return RESULT_PRESENT_FAILED;
    }
    if !framebuffer.present() {
        crate::error!("CPU1 framebuffer present failed");
        return RESULT_PRESENT_FAILED;
    }
    RESULT_OK
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

    if !framebuffer.wait_for_vsync() {
        crate::error!("CPU1 framebuffer VSYNC wait failed");
        return RESULT_PRESENT_FAILED;
    }
    if !framebuffer.present() {
        crate::error!("CPU1 framebuffer present failed");
        return RESULT_PRESENT_FAILED;
    }

    RESULT_OK
}
