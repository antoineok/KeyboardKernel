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

const LATE_TOLERANCE: Duration = Duration::from_millis(2);
const STATS_INTERVAL_FRAMES: u64 = 90;
/// Keep several already-decoded RGB frames ready so a costly IDR or fragment
/// read does not immediately turn into a visible presentation stall.
const PREPARED_QUEUE_FRAMES: usize = 3;

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

    /// Advance the current video frame when its deadline arrives.
    ///
    /// This does *not* advance the presentation clock. CPU1 still has to
    /// composite the selected frame and flip the framebuffer. `frame_presented`
    /// commits the duration only after that visible presentation completes.
    pub fn advance_if_due(&mut self, now: Duration) -> Result<bool, KwVideoError> {
        if self.pending_present_duration_ticks.is_some() || now < self.next_present_at {
            return Ok(false);
        }

        if self.prepared.is_empty() {
            self.queue_underruns = self.queue_underruns.saturating_add(1);
            self.prepare_next_frame()?;
        }

        let late = now > self.next_present_at + LATE_TOLERANCE;
        if late {
            self.late_frames = self.late_frames.saturating_add(1);
        }

        let next = self
            .prepared
            .pop_front()
            .expect("KWVIDEO prepared queue unexpectedly empty");
        if let Some(previous) = self.current.replace(next.frame) {
            self.spare_pixels.push(previous.into_pixels());
        }

        self.pending_present_duration_ticks = Some(next.duration_ticks);
        self.pending_present_was_late = late;
        Ok(true)
    }

    /// Commit one video frame after the compositor/page flip has completed.
    ///
    /// Anchoring the next deadline to the *actual* visible presentation time
    /// prevents composition cost from shortening a frame's on-screen lifetime.
    /// A 24000/1001 stream therefore keeps ~41.7 ms between completed flips
    /// even when composing the frame itself takes measurable CPU time.
    pub fn frame_presented(&mut self, presented_at: Duration) {
        let duration_ticks = self
            .pending_present_duration_ticks
            .take()
            .expect("KWVIDEO frame_presented without an advanced frame");

        if self.pending_present_was_late {
            self.clock_rebases = self.clock_rebases.saturating_add(1);
        }
        self.pending_present_was_late = false;

        self.presentation_ticks = self
            .presentation_ticks
            .checked_add(u64::from(duration_ticks))
            .expect("KWVIDEO presentation clock overflow");
        self.frames_presented = self.frames_presented.saturating_add(1);

        let frame_duration = self.ticks_to_duration(duration_ticks);
        self.next_present_at = presented_at
            .checked_add(frame_duration)
            .expect("KWVIDEO presentation deadline overflow");

        if self.frames_presented % STATS_INTERVAL_FRAMES == 0 {
            let media_ms = self.presentation_ticks.saturating_mul(1000) / u64::from(self.timescale);
            let wall_ms = presented_at.saturating_sub(self.started_at).as_millis();
            crate::info!(
                "CPU1 KWVIDEO playback: frames={}, media={} ms, wall={} ms, late={}, rebases={}, underruns={}, queued={}, fragment={}, loops={}",
                self.frames_presented,
                media_ms,
                wall_ms,
                self.late_frames,
                self.clock_rebases,
                self.queue_underruns,
                self.prepared.len(),
                self.fragment_index + 1,
                self.loops,
            );
        }
    }

    /// Decode at most one additional picture per graphics-scheduler pass.
    /// CPU1 therefore checks overlay/control commands between decode jobs while
    /// still using otherwise-idle time to maintain the presentation cushion.
    pub fn prepare_if_needed(&mut self) -> Result<(), KwVideoError> {
        if self.prepared.len() < PREPARED_QUEUE_FRAMES {
            self.prepare_next_frame()?;
        }
        Ok(())
    }

    fn ticks_to_duration(&self, ticks: u32) -> Duration {
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

        let cabac_ebsp = self.asset.picture_cabac_payload_bytes(sample_bytes, &picture)?;
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
        assert!(phase2.complete(), "CPU1 RPIVID phase 2 failed");

        let pixels = self.spare_pixels.pop().unwrap_or_default();
        let frame = RPIVID_WORKSPACE.copy_frame_rgbx_reuse(
            &picture,
            self.picture_index,
            pixels,
        );
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
