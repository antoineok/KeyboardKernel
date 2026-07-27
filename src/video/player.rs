//! CPU1-owned Keywork Video playback state.
//!
//! The graphics core keeps one displayed RGBX frame and one prepared RGBX
//! frame. After presenting frame N it immediately decodes/converts N+1, then
//! waits for that frame's media deadline while remaining able to service
//! graphics/overlay commands.

use alloc::vec::Vec;
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

pub struct VideoPlayer {
    asset: VideoAsset,
    fragment: MediaFragment,
    sample_index: usize,
    picture_index: usize,
    fragment_index: u64,
    timescale: u32,

    started_at: Duration,
    presentation_ticks: u64,
    prepared_duration_ticks: u32,
    prepared: Option<RpividDecodedFrame>,
    current: Option<RpividDecodedFrame>,
    spare_pixels: Vec<u8>,

    frames_presented: u64,
    late_frames: u64,
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
            prepared_duration_ticks: 0,
            prepared: None,
            current: None,
            spare_pixels: Vec::new(),
            frames_presented: 0,
            late_frames: 0,
            loops: 0,
        };
        player.prepare_next_frame()?;
        // Start the media clock only after frame zero is decoded and ready.
        // Asset open/decode latency must not make the entire stream late.
        player.started_at = time_manager().uptime();
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

    /// Present the prepared frame when its media deadline has arrived.
    ///
    /// Returns true when the visible background changed and CPU1 should
    /// recompose/present the framebuffer.
    pub fn present_if_due(&mut self, now: Duration) -> Result<bool, KwVideoError> {
        if self.prepared.is_none() {
            self.prepare_next_frame()?;
        }

        let deadline = self.presentation_deadline();
        let elapsed = now.saturating_sub(self.started_at);
        if elapsed < deadline {
            return Ok(false);
        }
        if elapsed > deadline + LATE_TOLERANCE {
            self.late_frames = self.late_frames.saturating_add(1);
        }

        let next = self
            .prepared
            .take()
            .expect("KWVIDEO prepared frame disappeared");
        if let Some(previous) = self.current.replace(next) {
            self.spare_pixels = previous.into_pixels();
        }

        self.presentation_ticks = self
            .presentation_ticks
            .checked_add(u64::from(self.prepared_duration_ticks))
            .expect("KWVIDEO presentation clock overflow");
        self.frames_presented = self.frames_presented.saturating_add(1);

        if self.frames_presented % STATS_INTERVAL_FRAMES == 0 {
            let media_ms = self.presentation_ticks.saturating_mul(1000) / u64::from(self.timescale);
            let wall_ms = now.saturating_sub(self.started_at).as_millis();
            crate::info!(
                "CPU1 KWVIDEO playback: frames={}, media={} ms, wall={} ms, late={}, fragment={}, loops={}",
                self.frames_presented,
                media_ms,
                wall_ms,
                self.late_frames,
                self.fragment_index + 1,
                self.loops,
            );
        }

        Ok(true)
    }

    /// Decode/convert the next picture when no prepared frame is waiting.
    /// Call this after rendering the current frame so scanout is never held
    /// hostage by preparation of the following picture.
    pub fn prepare_if_needed(&mut self) -> Result<(), KwVideoError> {
        self.prepare_next_frame()
    }

    fn presentation_deadline(&self) -> Duration {
        let nanos = (u128::from(self.presentation_ticks) * 1_000_000_000u128)
            / u128::from(self.timescale);
        Duration::from_nanos(nanos as u64)
    }

    fn prepare_next_frame(&mut self) -> Result<(), KwVideoError> {
        if self.prepared.is_some() {
            return Ok(());
        }
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

        let pixels = core::mem::take(&mut self.spare_pixels);
        self.prepared = Some(RPIVID_WORKSPACE.copy_frame_rgbx_reuse(
            &picture,
            self.picture_index,
            pixels,
        ));
        self.prepared_duration_ticks = sample.duration_ticks;
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
