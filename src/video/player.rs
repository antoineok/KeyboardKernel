//! CPU1-owned Keywork Video playback state.
//!
//! The graphics core keeps one displayed RGBX frame plus a small queue of
//! already-decoded RGBX frames. The queue absorbs expensive IDR/fragment
//! boundaries, while presentation is paced independently from decode work and
//! CPU1 remains able to service graphics/overlay commands between decode jobs.

use alloc::{collections::VecDeque, vec::Vec};
use core::time::Duration;

use crate::{
    storage::fs::vfs::{FsId, VFS},
    time::time_manager,
};

use super::{
    kwvideo::{FragmentSample, KwVideoError, MediaFragment, VideoAsset},
    rpivid::{RpividDecodedFrame, RPIVID_WORKSPACE},
};

const LATE_TOLERANCE: Duration = Duration::from_millis(20);
/// Keep several already-decoded RGB frames ready so a costly IDR or fragment
/// read does not immediately turn into a visible presentation stall.
const PREPARED_QUEUE_FRAMES: usize = 6;
const MIN_SAFE_QUEUED_FRAMES: usize = 2;
/// Bound each background storage transaction so CPU1 never spends a whole
/// frame period filling the resident compressed-video copy.
const RESIDENT_PREFETCH_BYTES_PER_PASS: usize = 4 * 1024;

struct PreparedFrame {
    frame: RpividDecodedFrame,
    duration_ticks: u32,
}

pub struct VideoPlayer {
    asset: VideoAsset,
    fragment: MediaFragment,
    sample_index: usize,
    picture_index: usize,
    fragment_index: u64,
    timescale: u32,

    started_at: Duration,
    presentation_ticks: u64,
    next_present_at: Duration,
    pending_present_duration_ticks: Option<u32>,
    pending_present_was_late: bool,
    prepared: VecDeque<PreparedFrame>,
    current: Option<RpividDecodedFrame>,
    spare_pixels: Vec<Vec<u8>>,

    frames_presented: u64,
    late_frames: u64,
    queue_underruns: u64,
    prepare_deferrals: u64,
    clock_rebases: u64,
    loops: u64,
}

impl VideoPlayer {
    pub fn open(fs: FsId, path: &str) -> Result<Self, KwVideoError> {
        let asset = VFS.with_ref(|vfs| VideoAsset::open(vfs, fs, path))?;
        let timescale = asset.metadata().timescale;
        let fragment = asset.cached_first_fragment()?;

        let mut player = Self {
            asset,
            fragment,
            sample_index: 0,
            picture_index: 0,
            fragment_index: 0,
            timescale,
            started_at: time_manager().uptime(),
            presentation_ticks: 0,
            next_present_at: time_manager().uptime(),
            pending_present_duration_ticks: None,
            pending_present_was_late: false,
            prepared: VecDeque::new(),
            current: None,
            spare_pixels: Vec::new(),
            frames_presented: 0,
            late_frames: 0,
            queue_underruns: 0,
            prepare_deferrals: 0,
            clock_rebases: 0,
            loops: 0,
        };
        // Build a small presentation cushion before starting the media clock.
        // Three RGB frames cost roughly 12 MiB at 1366x768 and absorb the
        // expensive IDR/fragment-boundary spikes we observed on hardware.
        while player.prepared.len() < PREPARED_QUEUE_FRAMES {
            player.prepare_next_frame()?;
        }
        let now = time_manager().uptime();
        player.started_at = now;
        player.next_present_at = now;
        Ok(player)
    }

    #[inline(always)]
    pub fn current_frame(&self) -> Option<&RpividDecodedFrame> {
        self.current.as_ref()
    }

    #[inline(always)]
    pub const fn frames_presented(&self) -> u64 {
        self.frames_presented
    }

    #[inline(always)]
    pub const fn late_frames(&self) -> u64 {
        self.late_frames
    }

    #[inline(always)]
    pub const fn loops(&self) -> u64 {
        self.loops
    }

    /// Select the next decoded picture for composition into the hidden
    /// framebuffer page.
    ///
    /// This deliberately happens *well before* the frame's presentation
    /// deadline. CPU1 can therefore perform the full-screen RGBX copy and all
    /// overlay work while the previous page is still scanning out. The actual
    /// deadline path then contains only VSYNC + page flip.
    pub fn stage_next_frame(&mut self) -> Result<bool, KwVideoError> {
        if self.pending_present_duration_ticks.is_some() {
            return Ok(false);
        }

        if self.prepared.is_empty() {
            self.queue_underruns = self.queue_underruns.saturating_add(1);
            self.prepare_next_frame()?;
        }

        let next = self
            .prepared
            .pop_front()
            .expect("KWVIDEO prepared queue unexpectedly empty");
        if let Some(previous) = self.current.replace(next.frame) {
            self.spare_pixels.push(previous.into_pixels());
        }

        self.pending_present_duration_ticks = Some(next.duration_ticks);
        self.pending_present_was_late = false;
        Ok(true)
    }

    /// Return true once it is time to block for the VSYNC that should expose
    /// the already-composed hidden framebuffer page.
    pub fn presentation_window_open(&self, now: Duration, presentation_lead: Duration) -> bool {
        if self.pending_present_duration_ticks.is_none() {
            return false;
        }

        now.checked_add(presentation_lead)
            .map(|ready_by| ready_by >= self.next_present_at)
            .unwrap_or(true)
    }

    /// Commit one frame at a real display presentation boundary.
    ///
    /// Normal playback stays on an absolute media timeline. A genuine large
    /// miss rebases that timeline so the player never repays lateness with a
    /// burst of too-fast frames.
    pub fn frame_presented(&mut self, presented_at: Duration) {
        let duration_ticks = self
            .pending_present_duration_ticks
            .take()
            .expect("KWVIDEO frame_presented without an advanced frame");

        let late = presented_at > self.next_present_at + LATE_TOLERANCE;
        if late {
            self.late_frames = self.late_frames.saturating_add(1);
        }
        self.pending_present_was_late = late;

        let rebase = self.pending_present_was_late;
        if rebase {
            self.clock_rebases = self.clock_rebases.saturating_add(1);
        }
        self.pending_present_was_late = false;

        self.presentation_ticks = self
            .presentation_ticks
            .checked_add(u64::from(duration_ticks))
            .expect("KWVIDEO presentation clock overflow");
        self.frames_presented = self.frames_presented.saturating_add(1);

        // Keep an absolute media timeline so a 24 fps source on a 60 Hz
        // display naturally alternates 3/2 vblanks instead of rounding every
        // frame up to three refreshes. Only rebase after a genuine (>20 ms)
        // scheduling miss, which prevents a later catch-up burst.
        if self.frames_presented == 1 {
            self.started_at = presented_at;
        } else if rebase {
            let elapsed_before_current = self
                .presentation_ticks
                .saturating_sub(u64::from(duration_ticks));
            self.started_at = presented_at
                .checked_sub(self.ticks_to_duration_u64(elapsed_before_current))
                .unwrap_or(Duration::ZERO);
        }
        self.next_present_at = self
            .started_at
            .checked_add(self.ticks_to_duration_u64(self.presentation_ticks))
            .expect("KWVIDEO presentation deadline overflow");
    }

    /// Decode at most one additional picture per graphics-scheduler pass.
    /// CPU1 therefore checks overlay/control commands between decode jobs while
    /// still using otherwise-idle time to maintain the presentation cushion.
    pub fn prepare_if_needed(
        &mut self,
        now: Duration,
        presentation_guard: Duration,
    ) -> Result<(), KwVideoError> {
        let presentation_imminent = self.pending_present_duration_ticks.is_some()
            && now
                .checked_add(presentation_guard)
                .map(|guard_end| guard_end >= self.next_present_at)
                .unwrap_or(true);

        // Fill the active video's RAM copy progressively while the display has
        // comfortable slack. This starts playback from the cached first
        // fragment immediately instead of blocking profile activation while
        // the complete .kwv file is copied from eMMC.
        if !presentation_imminent && !self.asset.resident_complete() {
            VFS.with_ref(|vfs| {
                self.asset
                    .prefetch_resident(vfs, RESIDENT_PREFETCH_BYTES_PER_PASS)
            })?;
        }

        if self.prepared.len() >= PREPARED_QUEUE_FRAMES {
            return Ok(());
        }

        // Never start an optional decode/conversion job when the already-
        // composed next page is entering its VSYNC presentation window. The
        // RPIVID submission path is currently synchronous, so a decode spike
        // here would prevent CPU1 from flipping a frame that is already ready.
        // Keep at least two queued pictures before deferring, so very high-FPS
        // streams degrade by missing a deadline rather than starving forever.
        if self.prepared.len() >= MIN_SAFE_QUEUED_FRAMES && presentation_imminent {
            self.prepare_deferrals = self.prepare_deferrals.saturating_add(1);
            return Ok(());
        }

        self.prepare_next_frame()?;
        Ok(())
    }

    fn ticks_to_duration(&self, ticks: u32) -> Duration {
        self.ticks_to_duration_u64(u64::from(ticks))
    }

    fn ticks_to_duration_u64(&self, ticks: u64) -> Duration {
        let nanos = (u128::from(ticks) * 1_000_000_000u128) / u128::from(self.timescale);
        Duration::from_nanos(nanos as u64)
    }

    fn prepare_next_frame(&mut self) -> Result<(), KwVideoError> {
        self.ensure_sample_available()?;

        let sample: FragmentSample = self.fragment.samples()[self.sample_index];
        let sample_bytes = self.fragment.sample_bytes(sample)?;
        let picture = self.asset.picture_syntax_bytes(sample_bytes)?;
        let slice = picture.slice;

        if self.picture_index == 0 {
            assert!(
                matches!(slice.nal_unit_type, 19 | 20) && slice.slice_type == 2,
                "each kwv1 fragment must begin with an IDR I-slice",
            );
        } else {
            assert_eq!(slice.slice_type, 1, "kwv1 inter pictures must be P slices");
        }

        let cabac_ebsp = self
            .asset
            .picture_cabac_payload_bytes(sample_bytes, &picture)?;
        let phase1 = RPIVID_WORKSPACE.submit_picture_phase1(&picture, cabac_ebsp);
        if !phase1.complete() {
            crate::error!(
                "CPU1 RPIVID phase1 failure: fragment={}, picture={}, POC={}, commands={}/{}, STATUS={:#010x}, BFSTATUS={:#010x}",
                self.fragment_index + 1,
                self.picture_index,
                slice.pic_order_cnt_lsb,
                phase1.cfstatus,
                phase1.cfnum,
                phase1.status,
                phase1.bfstatus,
            );
            panic!("CPU1 RPIVID phase 1 failed");
        }

        let phase2 = RPIVID_WORKSPACE.submit_picture_phase2(&picture, self.picture_index);
        if !phase2.complete() {
            crate::error!(
                "CPU1 RPIVID phase2 failure: fragment={}, picture={}, POC={}, STATUS2={:#010x}, intc={:#010x}",
                self.fragment_index + 1,
                self.picture_index,
                slice.pic_order_cnt_lsb,
                phase2.status2,
                phase2.interrupt_control,
            );
            panic!("CPU1 RPIVID phase 2 failed");
        }

        let pixels = self.spare_pixels.pop().unwrap_or_default();
        let frame = RPIVID_WORKSPACE.copy_frame_rgbx_reuse(&picture, self.picture_index, pixels);
        self.prepared.push_back(PreparedFrame {
            frame,
            duration_ticks: sample.duration_ticks,
        });
        self.sample_index += 1;
        self.picture_index += 1;
        Ok(())
    }

    fn ensure_sample_available(&mut self) -> Result<(), KwVideoError> {
        if self.sample_index < self.fragment.samples().len() {
            return Ok(());
        }

        let next = VFS.with_ref(|vfs| self.asset.read_next_fragment(vfs))?;
        self.fragment = match next {
            Some(fragment) => {
                self.fragment_index = self.fragment_index.saturating_add(1);
                fragment
            }
            None => {
                VFS.with_ref(|vfs| self.asset.rewind_stream(vfs))?;
                self.fragment_index = 0;
                self.loops = self.loops.saturating_add(1);
                self.asset.cached_first_fragment()?
            }
        };
        self.sample_index = 0;
        self.picture_index = 0;
        Ok(())
    }
}
