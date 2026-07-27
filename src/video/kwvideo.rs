//! Keywork Video (`kwv1`) profile validation.
//!
//! A Keywork Video file is still ordinary ISO-BMFF/MP4. The custom `kwv1`
//! major brand only promises that the file follows the constrained subset the
//! kernel supports.

use core::fmt;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::storage::fs::vfs::{FileStream, FsId, Vfs, VfsError};

use super::isobmff::{FourCc, IsoBmffError, StreamBoxReader};

pub const FTYP_BOX: FourCc = FourCc::new(*b"ftyp");
pub const MOOV_BOX: FourCc = FourCc::new(*b"moov");
pub const MOOF_BOX: FourCc = FourCc::new(*b"moof");
pub const MDAT_BOX: FourCc = FourCc::new(*b"mdat");
pub const KWVIDEO_BRAND: FourCc = FourCc::new(*b"kwv1");

/// Maximum permanently resident transition bootstrap for one profile video.
///
/// `kwv1` fragments on every one-second keyframe interval, so this normally
/// contains the initialization segment plus roughly one second of compressed
/// HEVC. Even with nine profile/fallback videos, the hard ceiling remains
/// bounded instead of letting media assets consume the complete kernel heap.
pub const MAX_TRANSITION_BOOTSTRAP_BYTES: usize = 4 * 1024 * 1024;
/// Maximum compressed KWVIDEO file kept resident for stutter-free playback.
/// The active video is read from eMMC once when playback starts; fragment
/// iteration thereafter is memory-only.
pub const MAX_RESIDENT_KWVIDEO_BYTES: usize = 64 * 1024 * 1024;

/// Eight configurable profiles plus the optional fallback visual profile.
pub const VIDEO_PROFILE_SLOT_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileType {
    major_brand: FourCc,
    minor_version: u32,
    compatible_brand_count: usize,
}

impl FileType {
    #[inline(always)]
    pub const fn major_brand(self) -> FourCc {
        self.major_brand
    }

    #[inline(always)]
    pub const fn minor_version(self) -> u32 {
        self.minor_version
    }

    #[inline(always)]
    pub const fn compatible_brand_count(self) -> usize {
        self.compatible_brand_count
    }
}

/// Validate the leading `ftyp` box of a Keywork Video stream.
///
/// The major brand carries the Keywork profile generation (`kwv1`). The MP4
/// minor-version field is retained as metadata but deliberately not used for
/// Keywork compatibility decisions; regular MP4 muxers choose that value.
pub fn parse_file_type(
    vfs: &Vfs<'_>,
    stream: &mut FileStream,
) -> Result<FileType, KwVideoError> {
    let mut boxes = StreamBoxReader::new(vfs, stream);
    let header = boxes.next_header()?.ok_or(KwVideoError::EmptyFile)?;
    drop(boxes);

    if header.box_type() != FTYP_BOX {
        return Err(KwVideoError::MissingFileType(header.box_type()));
    }

    let payload_size = header.payload_size();

    if payload_size < 8 {
        return Err(KwVideoError::InvalidFileTypeSize(payload_size));
    }

    if payload_size & 3 != 0 {
        return Err(KwVideoError::MisalignedFileTypeSize(payload_size));
    }

    let mut fixed = [0u8; 8];
    vfs.read_exact_stream(stream, &mut fixed)?;

    let major_brand = FourCc::new([fixed[0], fixed[1], fixed[2], fixed[3]]);
    let minor_version = u32::from_be_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]);

    if major_brand != KWVIDEO_BRAND {
        return Err(KwVideoError::UnsupportedMajorBrand(major_brand));
    }

    let compatible_brand_count = (payload_size - fixed.len()) / 4;

    for _ in 0..compatible_brand_count {
        let mut brand = [0u8; 4];
        vfs.read_exact_stream(stream, &mut brand)?;
    }

    Ok(FileType {
        major_brand,
        minor_version,
        compatible_brand_count,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HevcConfiguration {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub nal_length_size: u8,
    pub vps_count: u16,
    pub sps_count: u16,
    pub pps_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstFragment {
    pub base_decode_time: u64,
    pub sample_count: u32,
    pub duration_ticks: u64,
    pub first_sample_offset: usize,
    pub first_sample_size: u32,
    pub total_sample_bytes: usize,
    pub first_sample_sync: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FragmentSample {
    pub offset: usize,
    pub size: u32,
    pub duration_ticks: u32,
    pub sync: bool,
}

#[derive(Debug)]
pub struct MediaFragment {
    file_offset: usize,
    bytes: Box<[u8]>,
    base_decode_time: u64,
    duration_ticks: u64,
    samples: Vec<FragmentSample>,
}

impl MediaFragment {
    #[inline(always)]
    pub const fn base_decode_time(&self) -> u64 {
        self.base_decode_time
    }

    #[inline(always)]
    pub const fn duration_ticks(&self) -> u64 {
        self.duration_ticks
    }

    #[inline(always)]
    pub fn samples(&self) -> &[FragmentSample] {
        &self.samples
    }

    pub fn sample_bytes(&self, sample: FragmentSample) -> Result<&[u8], KwVideoError> {
        let start = sample
            .offset
            .checked_sub(self.file_offset)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let end = start
            .checked_add(sample.size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
        self.bytes
            .get(start..end)
            .ok_or(KwVideoError::InvalidStructure(
                "HEVC sample escapes media fragment",
            ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoMetadata {
    pub track_id: u32,
    pub width: u16,
    pub height: u16,
    pub timescale: u32,
    pub hevc: HevcConfiguration,
    pub first_fragment: FirstFragment,
}

#[derive(Debug)]
pub struct FirstAccessUnit {
    bytes: Box<[u8]>,
    parameter_set_count: u16,
    sample_nal_count: u16,
    vcl_nal_count: u16,
    first_vcl_type: u8,
}

impl FirstAccessUnit {
    #[inline(always)]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline(always)]
    pub const fn parameter_set_count(&self) -> u16 {
        self.parameter_set_count
    }

    #[inline(always)]
    pub const fn sample_nal_count(&self) -> u16 {
        self.sample_nal_count
    }

    #[inline(always)]
    pub const fn vcl_nal_count(&self) -> u16 {
        self.vcl_nal_count
    }

    #[inline(always)]
    pub const fn first_vcl_type(&self) -> u8 {
        self.first_vcl_type
    }
}

#[derive(Clone, Copy)]
struct MemoryBox<'a> {
    box_type: FourCc,
    start: usize,
    payload_start: usize,
    payload: &'a [u8],
}

fn next_memory_box<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    base: usize,
) -> Result<Option<MemoryBox<'a>>, KwVideoError> {
    if *cursor == bytes.len() {
        return Ok(None);
    }
    if *cursor > bytes.len() {
        return Err(KwVideoError::InvalidStructure("box cursor escaped container"));
    }

    let remaining = &bytes[*cursor..];
    let header = super::isobmff::BoxHeader::parse(remaining, remaining.len())?;
    let start = *cursor;
    let payload_start = start
        .checked_add(header.header_size())
        .ok_or(KwVideoError::OffsetOverflow)?;
    let end = start
        .checked_add(header.box_size())
        .ok_or(KwVideoError::OffsetOverflow)?;
    *cursor = end;

    Ok(Some(MemoryBox {
        box_type: header.box_type(),
        start: base.checked_add(start).ok_or(KwVideoError::OffsetOverflow)?,
        payload_start: base
            .checked_add(payload_start)
            .ok_or(KwVideoError::OffsetOverflow)?,
        payload: &bytes[payload_start..end],
    }))
}

fn find_box<'a>(
    bytes: &'a [u8],
    base: usize,
    wanted: FourCc,
) -> Result<MemoryBox<'a>, KwVideoError> {
    let mut cursor = 0;
    while let Some(current) = next_memory_box(bytes, &mut cursor, base)? {
        if current.box_type == wanted {
            return Ok(current);
        }
    }
    Err(KwVideoError::InvalidStructure("required MP4 box missing"))
}

fn count_boxes(bytes: &[u8], base: usize, wanted: FourCc) -> Result<usize, KwVideoError> {
    let mut cursor = 0;
    let mut count = 0;
    while let Some(current) = next_memory_box(bytes, &mut cursor, base)? {
        if current.box_type == wanted {
            count += 1;
        }
    }
    Ok(count)
}

fn parse_video_metadata(
    bootstrap: &[u8],
    first_fragment_offset: usize,
) -> Result<VideoMetadata, KwVideoError> {
    let moov = find_box(bootstrap, 0, FourCc::new(*b"moov"))?;
    if count_boxes(moov.payload, moov.payload_start, FourCc::new(*b"trak"))? != 1 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 requires exactly one track",
        ));
    }

    let trak = find_box(moov.payload, moov.payload_start, FourCc::new(*b"trak"))?;
    let tkhd = find_box(trak.payload, trak.payload_start, FourCc::new(*b"tkhd"))?;
    let (track_id, tkhd_width, tkhd_height) = parse_tkhd(tkhd.payload)?;

    let mdia = find_box(trak.payload, trak.payload_start, FourCc::new(*b"mdia"))?;
    let hdlr = find_box(mdia.payload, mdia.payload_start, FourCc::new(*b"hdlr"))?;
    if hdlr.payload.len() < 12 || &hdlr.payload[8..12] != b"vide" {
        return Err(KwVideoError::UnsupportedProfile("kwv1 track is not video"));
    }

    let mdhd = find_box(mdia.payload, mdia.payload_start, FourCc::new(*b"mdhd"))?;
    let timescale = parse_mdhd_timescale(mdhd.payload)?;

    let minf = find_box(mdia.payload, mdia.payload_start, FourCc::new(*b"minf"))?;
    let stbl = find_box(minf.payload, minf.payload_start, FourCc::new(*b"stbl"))?;
    let stsd = find_box(stbl.payload, stbl.payload_start, FourCc::new(*b"stsd"))?;
    let (width, height, hevc) = parse_stsd(stsd.payload, stsd.payload_start)?;

    let _display_dimensions = (tkhd_width, tkhd_height);
    if width == 0 || height == 0 || width > 1366 || height > 768 {
        return Err(KwVideoError::UnsupportedProfile(
            "video dimensions exceed kwv1 limits",
        ));
    }

    let moof = find_box(bootstrap, 0, FourCc::new(*b"moof"))?;
    if moof.start != first_fragment_offset {
        return Err(KwVideoError::InvalidStructure(
            "cached first fragment offset does not match first moof",
        ));
    }
    let moof_end = moof
        .payload_start
        .checked_add(moof.payload.len())
        .ok_or(KwVideoError::OffsetOverflow)?;
    let mdat = find_box(&bootstrap[moof_end..], moof_end, FourCc::new(*b"mdat"))?;
    let first_fragment = parse_first_fragment(moof, mdat, track_id)?;

    Ok(VideoMetadata {
        track_id,
        width,
        height,
        timescale,
        hevc,
        first_fragment,
    })
}

fn parse_tkhd(bytes: &[u8]) -> Result<(u32, u16, u16), KwVideoError> {
    let track_id_offset = match bytes.first().copied() {
        Some(0) => 12,
        Some(1) => 20,
        _ => return Err(KwVideoError::InvalidStructure("unsupported tkhd version")),
    };
    if bytes.len() < track_id_offset + 4 || bytes.len() < 8 {
        return Err(KwVideoError::InvalidStructure("truncated tkhd"));
    }

    let track_id = read_u32(bytes, track_id_offset)?;
    let width = (read_u32(bytes, bytes.len() - 8)? >> 16) as u16;
    let height = (read_u32(bytes, bytes.len() - 4)? >> 16) as u16;
    Ok((track_id, width, height))
}

fn parse_mdhd_timescale(bytes: &[u8]) -> Result<u32, KwVideoError> {
    let offset = match bytes.first().copied() {
        Some(0) => 12,
        Some(1) => 20,
        _ => return Err(KwVideoError::InvalidStructure("unsupported mdhd version")),
    };
    let timescale = read_u32(bytes, offset)?;
    if timescale == 0 {
        return Err(KwVideoError::InvalidStructure("zero media timescale"));
    }
    Ok(timescale)
}

fn parse_stsd(
    bytes: &[u8],
    base: usize,
) -> Result<(u16, u16, HevcConfiguration), KwVideoError> {
    if bytes.len() < 8 || read_u32(bytes, 4)? != 1 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 requires one sample description",
        ));
    }

    let entries = &bytes[8..];
    let entry = find_box(entries, base + 8, FourCc::new(*b"hvc1"))?;
    if entry.payload.len() < 78 {
        return Err(KwVideoError::InvalidStructure("truncated hvc1 sample entry"));
    }

    let width = read_u16(entry.payload, 24)?;
    let height = read_u16(entry.payload, 26)?;
    let hvcc = find_box(
        &entry.payload[78..],
        entry.payload_start + 78,
        FourCc::new(*b"hvcC"),
    )?;
    let hevc = parse_hvcc(hvcc.payload)?;
    Ok((width, height, hevc))
}

fn parse_hvcc(bytes: &[u8]) -> Result<HevcConfiguration, KwVideoError> {
    if bytes.len() < 23 || bytes[0] != 1 {
        return Err(KwVideoError::InvalidStructure("invalid hvcC header"));
    }

    let profile_idc = bytes[1] & 0x1f;
    let level_idc = bytes[12];
    let chroma_format_idc = bytes[16] & 0x03;
    let bit_depth_luma = 8 + (bytes[17] & 0x07);
    let bit_depth_chroma = 8 + (bytes[18] & 0x07);
    let nal_length_size = 1 + (bytes[21] & 0x03);

    if profile_idc != 1
        || chroma_format_idc != 1
        || bit_depth_luma != 8
        || bit_depth_chroma != 8
        || nal_length_size != 4
    {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 requires HEVC Main 8-bit 4:2:0 with 4-byte NAL lengths",
        ));
    }

    let mut cursor = 23usize;
    let mut vps_count = 0u16;
    let mut sps_count = 0u16;
    let mut pps_count = 0u16;
    for _ in 0..bytes[22] {
        let nal_type = *bytes
            .get(cursor)
            .ok_or(KwVideoError::InvalidStructure("truncated hvcC array"))?
            & 0x3f;
        cursor += 1;
        let count = read_u16(bytes, cursor)?;
        cursor += 2;
        match nal_type {
            32 => vps_count = vps_count.saturating_add(count),
            33 => sps_count = sps_count.saturating_add(count),
            34 => pps_count = pps_count.saturating_add(count),
            _ => {}
        }
        for _ in 0..count {
            let length = read_u16(bytes, cursor)? as usize;
            cursor += 2;
            cursor = cursor
                .checked_add(length)
                .ok_or(KwVideoError::OffsetOverflow)?;
            if cursor > bytes.len() {
                return Err(KwVideoError::InvalidStructure("NAL exceeds hvcC payload"));
            }
        }
    }
    if vps_count == 0 || sps_count == 0 || pps_count == 0 {
        return Err(KwVideoError::InvalidStructure("hvcC is missing VPS/SPS/PPS"));
    }

    Ok(HevcConfiguration {
        profile_idc,
        level_idc,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        nal_length_size,
        vps_count,
        sps_count,
        pps_count,
    })
}

fn parse_first_fragment(
    moof: MemoryBox<'_>,
    mdat: MemoryBox<'_>,
    track_id: u32,
) -> Result<FirstFragment, KwVideoError> {
    let traf = find_box(moof.payload, moof.payload_start, FourCc::new(*b"traf"))?;
    let tfhd = find_box(traf.payload, traf.payload_start, FourCc::new(*b"tfhd"))?;
    let defaults = parse_tfhd(tfhd.payload, track_id)?;
    let tfdt = find_box(traf.payload, traf.payload_start, FourCc::new(*b"tfdt"))?;
    let base_decode_time = parse_tfdt(tfdt.payload)?;
    if count_boxes(traf.payload, traf.payload_start, FourCc::new(*b"trun"))? != 1 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 requires one trun per fragment",
        ));
    }
    let trun = find_box(traf.payload, traf.payload_start, FourCc::new(*b"trun"))?;
    parse_trun(
        trun.payload,
        defaults,
        moof.start,
        mdat.payload_start,
        mdat.payload.len(),
        base_decode_time,
    )
}

fn parse_first_fragment_samples(
    bootstrap: &[u8],
    first_fragment_offset: usize,
    track_id: u32,
) -> Result<Vec<FragmentSample>, KwVideoError> {
    let moof = find_box(bootstrap, 0, FourCc::new(*b"moof"))?;
    if moof.start != first_fragment_offset {
        return Err(KwVideoError::InvalidStructure(
            "cached first fragment offset does not match first moof",
        ));
    }
    let moof_end = moof
        .payload_start
        .checked_add(moof.payload.len())
        .ok_or(KwVideoError::OffsetOverflow)?;
    let mdat = find_box(&bootstrap[moof_end..], moof_end, FourCc::new(*b"mdat"))?;
    let traf = find_box(moof.payload, moof.payload_start, FourCc::new(*b"traf"))?;
    let tfhd = find_box(traf.payload, traf.payload_start, FourCc::new(*b"tfhd"))?;
    let defaults = parse_tfhd(tfhd.payload, track_id)?;
    let trun = find_box(traf.payload, traf.payload_start, FourCc::new(*b"trun"))?;
    parse_trun_samples(
        trun.payload,
        defaults,
        moof.start,
        mdat.payload_start,
        mdat.payload.len(),
    )
}

#[derive(Clone, Copy)]
struct FragmentDefaults {
    duration: Option<u32>,
    size: Option<u32>,
    flags: Option<u32>,
}

fn parse_tfhd(bytes: &[u8], expected_track: u32) -> Result<FragmentDefaults, KwVideoError> {
    if bytes.len() < 8 {
        return Err(KwVideoError::InvalidStructure("truncated tfhd"));
    }
    let flags = read_u32(bytes, 0)? & 0x00ff_ffff;
    if read_u32(bytes, 4)? != expected_track {
        return Err(KwVideoError::InvalidStructure("fragment track ID mismatch"));
    }
    if flags & 0x020000 == 0 || flags & 0x000001 != 0 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 fragments must use default-base-is-moof",
        ));
    }

    let mut cursor = 8usize;
    if flags & 0x000002 != 0 {
        cursor += 4;
    }
    let duration = if flags & 0x000008 != 0 {
        let value = read_u32(bytes, cursor)?;
        cursor += 4;
        Some(value)
    } else {
        None
    };
    let size = if flags & 0x000010 != 0 {
        let value = read_u32(bytes, cursor)?;
        cursor += 4;
        Some(value)
    } else {
        None
    };
    let sample_flags = if flags & 0x000020 != 0 {
        Some(read_u32(bytes, cursor)?)
    } else {
        None
    };

    if cursor > bytes.len() {
        return Err(KwVideoError::InvalidStructure("truncated tfhd"));
    }

    Ok(FragmentDefaults {
        duration,
        size,
        flags: sample_flags,
    })
}

fn parse_tfdt(bytes: &[u8]) -> Result<u64, KwVideoError> {
    match bytes.first().copied() {
        Some(0) => Ok(read_u32(bytes, 4)? as u64),
        Some(1) => read_u64(bytes, 4),
        _ => Err(KwVideoError::InvalidStructure("unsupported tfdt version")),
    }
}

fn parse_trun(
    bytes: &[u8],
    defaults: FragmentDefaults,
    moof_start: usize,
    mdat_start: usize,
    mdat_size: usize,
    base_decode_time: u64,
) -> Result<FirstFragment, KwVideoError> {
    if bytes.len() < 12 {
        return Err(KwVideoError::InvalidStructure("truncated trun"));
    }
    let flags = read_u32(bytes, 0)? & 0x00ff_ffff;
    if flags & 0x000001 == 0 || flags & 0x000800 != 0 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 trun requires data_offset and no composition offsets",
        ));
    }

    let sample_count = read_u32(bytes, 4)?;
    if sample_count == 0 {
        return Err(KwVideoError::InvalidStructure("empty trun"));
    }
    let data_offset = read_u32(bytes, 8)? as i32;
    let first_sample_offset = (moof_start as i128) + (data_offset as i128);
    if first_sample_offset < 0 || first_sample_offset > usize::MAX as i128 {
        return Err(KwVideoError::OffsetOverflow);
    }
    let first_sample_offset = first_sample_offset as usize;

    let mut cursor = 12usize;
    let first_sample_flags = if flags & 0x000004 != 0 {
        let value = read_u32(bytes, cursor)?;
        cursor += 4;
        Some(value)
    } else {
        None
    };
    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;

    let mut duration_ticks = 0u64;
    let mut total_sample_bytes = 0usize;
    let mut first_sample_size = 0u32;
    let mut first_flags = first_sample_flags;

    for index in 0..sample_count {
        let duration = if has_duration {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            value
        } else {
            defaults
                .duration
                .ok_or(KwVideoError::InvalidStructure("sample duration missing"))?
        };
        let size = if has_size {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            value
        } else {
            defaults
                .size
                .ok_or(KwVideoError::InvalidStructure("sample size missing"))?
        };
        let sample_flags = if has_flags {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            Some(value)
        } else {
            defaults.flags
        };

        if index == 0 {
            first_sample_size = size;
            if first_flags.is_none() {
                first_flags = sample_flags;
            }
        }
        duration_ticks = duration_ticks
            .checked_add(duration as u64)
            .ok_or(KwVideoError::OffsetOverflow)?;
        total_sample_bytes = total_sample_bytes
            .checked_add(size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
    }

    let first_sample_sync = first_flags
        .map(|value| value & 0x00010000 == 0)
        .ok_or(KwVideoError::InvalidStructure("first sample flags missing"))?;
    if !first_sample_sync {
        return Err(KwVideoError::UnsupportedProfile(
            "first fragment must begin with a sync sample",
        ));
    }

    let mdat_end = mdat_start
        .checked_add(mdat_size)
        .ok_or(KwVideoError::OffsetOverflow)?;
    let samples_end = first_sample_offset
        .checked_add(total_sample_bytes)
        .ok_or(KwVideoError::OffsetOverflow)?;
    if first_sample_offset < mdat_start || samples_end > mdat_end {
        return Err(KwVideoError::InvalidStructure(
            "fragment sample bytes escape first mdat",
        ));
    }

    Ok(FirstFragment {
        base_decode_time,
        sample_count,
        duration_ticks,
        first_sample_offset,
        first_sample_size,
        total_sample_bytes,
        first_sample_sync,
    })
}

fn parse_trun_samples(
    bytes: &[u8],
    defaults: FragmentDefaults,
    moof_start: usize,
    mdat_start: usize,
    mdat_size: usize,
) -> Result<Vec<FragmentSample>, KwVideoError> {
    if bytes.len() < 12 {
        return Err(KwVideoError::InvalidStructure("truncated trun"));
    }
    let flags = read_u32(bytes, 0)? & 0x00ff_ffff;
    if flags & 0x000001 == 0 || flags & 0x000800 != 0 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 trun requires data_offset and no composition offsets",
        ));
    }

    let sample_count = read_u32(bytes, 4)? as usize;
    if sample_count == 0 {
        return Err(KwVideoError::InvalidStructure("empty trun"));
    }
    let data_offset = read_u32(bytes, 8)? as i32;
    let first_sample_offset = (moof_start as i128) + (data_offset as i128);
    if first_sample_offset < 0 || first_sample_offset > usize::MAX as i128 {
        return Err(KwVideoError::OffsetOverflow);
    }

    let mut cursor = 12usize;
    let first_sample_flags = if flags & 0x000004 != 0 {
        let value = read_u32(bytes, cursor)?;
        cursor += 4;
        Some(value)
    } else {
        None
    };
    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;

    let mut sample_offset = first_sample_offset as usize;
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let duration_ticks = if has_duration {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            value
        } else {
            defaults
                .duration
                .ok_or(KwVideoError::InvalidStructure("sample duration missing"))?
        };
        let size = if has_size {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            value
        } else {
            defaults
                .size
                .ok_or(KwVideoError::InvalidStructure("sample size missing"))?
        };
        let flags = if index == 0 {
            first_sample_flags.or_else(|| {
                if has_flags {
                    None
                } else {
                    defaults.flags
                }
            })
        } else if has_flags {
            None
        } else {
            defaults.flags
        };
        let sample_flags = if has_flags {
            let value = read_u32(bytes, cursor)?;
            cursor += 4;
            if index == 0 {
                first_sample_flags.unwrap_or(value)
            } else {
                value
            }
        } else {
            flags.ok_or(KwVideoError::InvalidStructure("sample flags missing"))?
        };
        if size == 0 {
            return Err(KwVideoError::InvalidStructure("zero-sized fragment sample"));
        }
        samples.push(FragmentSample {
            offset: sample_offset,
            size,
            duration_ticks,
            sync: sample_flags & 0x00010000 == 0,
        });
        sample_offset = sample_offset
            .checked_add(size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
    }

    let mdat_end = mdat_start
        .checked_add(mdat_size)
        .ok_or(KwVideoError::OffsetOverflow)?;
    if samples[0].offset < mdat_start || sample_offset > mdat_end {
        return Err(KwVideoError::InvalidStructure(
            "fragment sample bytes escape first mdat",
        ));
    }
    Ok(samples)
}

fn parse_media_fragment(
    bytes: Box<[u8]>,
    file_offset: usize,
    track_id: u32,
) -> Result<MediaFragment, KwVideoError> {
    let moof = find_box(&bytes, file_offset, MOOF_BOX)?;
    if moof.start != file_offset {
        return Err(KwVideoError::InvalidStructure(
            "media fragment does not begin with moof",
        ));
    }
    let moof_end = moof
        .payload_start
        .checked_add(moof.payload.len())
        .ok_or(KwVideoError::OffsetOverflow)?;
    let local_moof_end = moof_end
        .checked_sub(file_offset)
        .ok_or(KwVideoError::OffsetOverflow)?;
    let mdat = find_box(&bytes[local_moof_end..], moof_end, MDAT_BOX)?;

    let summary = parse_first_fragment(moof, mdat, track_id)?;
    let samples = {
        let traf = find_box(moof.payload, moof.payload_start, FourCc::new(*b"traf"))?;
        let tfhd = find_box(traf.payload, traf.payload_start, FourCc::new(*b"tfhd"))?;
        let defaults = parse_tfhd(tfhd.payload, track_id)?;
        let trun = find_box(traf.payload, traf.payload_start, FourCc::new(*b"trun"))?;
        parse_trun_samples(
            trun.payload,
            defaults,
            moof.start,
            mdat.payload_start,
            mdat.payload.len(),
        )?
    };

    Ok(MediaFragment {
        file_offset,
        bytes,
        base_decode_time: summary.base_decode_time,
        duration_ticks: summary.duration_ticks,
        samples,
    })
}

fn find_hvcc_in_bootstrap(bootstrap: &[u8]) -> Result<MemoryBox<'_>, KwVideoError> {
    let moov = find_box(bootstrap, 0, FourCc::new(*b"moov"))?;
    let trak = find_box(moov.payload, moov.payload_start, FourCc::new(*b"trak"))?;
    let mdia = find_box(trak.payload, trak.payload_start, FourCc::new(*b"mdia"))?;
    let minf = find_box(mdia.payload, mdia.payload_start, FourCc::new(*b"minf"))?;
    let stbl = find_box(minf.payload, minf.payload_start, FourCc::new(*b"stbl"))?;
    let stsd = find_box(stbl.payload, stbl.payload_start, FourCc::new(*b"stsd"))?;
    if stsd.payload.len() < 8 {
        return Err(KwVideoError::InvalidStructure("truncated stsd"));
    }
    let entry = find_box(
        &stsd.payload[8..],
        stsd.payload_start + 8,
        FourCc::new(*b"hvc1"),
    )?;
    if entry.payload.len() < 78 {
        return Err(KwVideoError::InvalidStructure("truncated hvc1 sample entry"));
    }
    find_box(
        &entry.payload[78..],
        entry.payload_start + 78,
        FourCc::new(*b"hvcC"),
    )
}

fn append_annex_b_nal(destination: &mut Vec<u8>, nal: &[u8]) -> Result<(), KwVideoError> {
    if nal.len() < 2 {
        return Err(KwVideoError::InvalidStructure("truncated HEVC NAL unit"));
    }
    destination.extend_from_slice(&[0, 0, 0, 1]);
    destination.extend_from_slice(nal);
    Ok(())
}

fn append_parameter_sets(
    hvcc: &[u8],
    destination: &mut Vec<u8>,
) -> Result<u16, KwVideoError> {
    if hvcc.len() < 23 {
        return Err(KwVideoError::InvalidStructure("truncated hvcC"));
    }

    let mut cursor = 23usize;
    let mut appended = 0u16;
    for _ in 0..hvcc[22] {
        let nal_type = *hvcc
            .get(cursor)
            .ok_or(KwVideoError::InvalidStructure("truncated hvcC array"))?
            & 0x3f;
        cursor += 1;
        let count = read_u16(hvcc, cursor)?;
        cursor += 2;
        for _ in 0..count {
            let length = read_u16(hvcc, cursor)? as usize;
            cursor += 2;
            let end = cursor
                .checked_add(length)
                .ok_or(KwVideoError::OffsetOverflow)?;
            let nal = hvcc
                .get(cursor..end)
                .ok_or(KwVideoError::InvalidStructure("NAL exceeds hvcC payload"))?;
            if matches!(nal_type, 32..=34) {
                append_annex_b_nal(destination, nal)?;
                appended = appended.saturating_add(1);
            }
            cursor = end;
        }
    }

    if appended == 0 {
        return Err(KwVideoError::InvalidStructure(
            "hvcC contains no decodable parameter sets",
        ));
    }
    Ok(appended)
}

fn append_length_prefixed_sample(
    sample: &[u8],
    nal_length_size: usize,
    destination: &mut Vec<u8>,
) -> Result<(u16, u16, u8), KwVideoError> {
    if nal_length_size != 4 {
        return Err(KwVideoError::UnsupportedProfile(
            "kwv1 access units require 4-byte NAL lengths",
        ));
    }

    let mut cursor = 0usize;
    let mut nal_count = 0u16;
    let mut vcl_count = 0u16;
    let mut first_vcl_type = None;

    while cursor < sample.len() {
        let length = read_u32(sample, cursor)? as usize;
        cursor += nal_length_size;
        if length == 0 {
            return Err(KwVideoError::InvalidStructure("zero-length HEVC NAL unit"));
        }
        let end = cursor
            .checked_add(length)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let nal = sample
            .get(cursor..end)
            .ok_or(KwVideoError::InvalidStructure("HEVC NAL exceeds sample"))?;
        if nal.len() < 2 {
            return Err(KwVideoError::InvalidStructure("truncated HEVC NAL unit"));
        }
        let nal_type = (nal[0] >> 1) & 0x3f;
        if nal_type <= 31 {
            vcl_count = vcl_count.saturating_add(1);
            if first_vcl_type.is_none() {
                first_vcl_type = Some(nal_type);
            }
        }
        append_annex_b_nal(destination, nal)?;
        nal_count = nal_count.saturating_add(1);
        cursor = end;
    }

    if nal_count == 0 {
        return Err(KwVideoError::InvalidStructure("empty HEVC sample"));
    }
    let first_vcl_type = first_vcl_type.ok_or(KwVideoError::InvalidStructure(
        "first HEVC sample contains no VCL NAL",
    ))?;
    Ok((nal_count, vcl_count, first_vcl_type))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, KwVideoError> {
    let end = offset.checked_add(2).ok_or(KwVideoError::OffsetOverflow)?;
    let data = bytes
        .get(offset..end)
        .ok_or(KwVideoError::InvalidStructure("truncated u16"))?;
    Ok(u16::from_be_bytes([data[0], data[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, KwVideoError> {
    let end = offset.checked_add(4).ok_or(KwVideoError::OffsetOverflow)?;
    let data = bytes
        .get(offset..end)
        .ok_or(KwVideoError::InvalidStructure("truncated u32"))?;
    Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, KwVideoError> {
    let end = offset.checked_add(8).ok_or(KwVideoError::OffsetOverflow)?;
    let data = bytes
        .get(offset..end)
        .ok_or(KwVideoError::InvalidStructure("truncated u64"))?;
    Ok(u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ]))
}


/// A `kwv1` asset prepared for instant profile transitions.
///
/// The complete video stays on FAT/eMMC. Only the ISO-BMFF initialization
/// segment and first media fragment are pinned in RAM. Because the host muxer
/// starts every fragment with a keyframe, that cached fragment is sufficient to
/// begin decoding immediately while the file stream is already positioned at
/// the next fragment.
pub struct VideoAsset {
    file_type: FileType,
    metadata: VideoMetadata,
    stream: FileStream,
    bootstrap: Box<[u8]>,
    resident: Box<[u8]>,
    resident_position: usize,
    first_fragment_offset: usize,
    resume_offset: usize,
}

impl VideoAsset {
    /// Open and prepare one video asset from a mounted filesystem.
    pub fn open(
        vfs: &Vfs<'_>,
        fs: FsId,
        path: &str,
    ) -> Result<Self, KwVideoError> {
        let stream = vfs.open_stream_fs(fs, path)?;
        Self::from_stream(vfs, stream)
    }

    /// Prepare an already-open stream and pin its transition bootstrap in RAM.
    pub fn from_stream(
        vfs: &Vfs<'_>,
        mut stream: FileStream,
    ) -> Result<Self, KwVideoError> {
        let file_type = parse_file_type(vfs, &mut stream)?;
        let mut boxes = StreamBoxReader::new(vfs, &mut stream);
        let mut saw_movie = false;
        let mut first_fragment_offset = None;
        let resume_offset;

        loop {
            let header = boxes
                .next_header()?
                .ok_or(KwVideoError::MissingFirstMediaFragment)?;
            let box_start = boxes
                .position()
                .checked_sub(header.header_size())
                .ok_or(KwVideoError::OffsetOverflow)?;

            match header.box_type() {
                MOOV_BOX => {
                    if first_fragment_offset.is_some() {
                        return Err(KwVideoError::MovieBoxAfterFragment);
                    }
                    saw_movie = true;
                }
                MOOF_BOX => {
                    if !saw_movie {
                        return Err(KwVideoError::FragmentBeforeMovieBox);
                    }

                    if first_fragment_offset.is_none() {
                        first_fragment_offset = Some(box_start);
                    }
                }
                MDAT_BOX if first_fragment_offset.is_some() => {
                    boxes.skip_payload(header)?;
                    resume_offset = boxes.position();
                    break;
                }
                MDAT_BOX => return Err(KwVideoError::MediaDataBeforeFragment),
                _ => {}
            }

            boxes.skip_payload(header)?;
        }

        let first_fragment_offset =
            first_fragment_offset.ok_or(KwVideoError::MissingFirstMediaFragment)?;

        if resume_offset > MAX_TRANSITION_BOOTSTRAP_BYTES {
            return Err(KwVideoError::TransitionBootstrapTooLarge {
                required: resume_offset,
                maximum: MAX_TRANSITION_BOOTSTRAP_BYTES,
            });
        }

        drop(boxes);
        vfs.seek_stream(&mut stream, 0)?;

        let mut bootstrap = vec![0u8; resume_offset];
        vfs.read_exact_stream(&mut stream, &mut bootstrap)?;

        debug_assert_eq!(stream.position(), resume_offset);

        let metadata = parse_video_metadata(&bootstrap, first_fragment_offset)?;

        let file_len = stream.len();
        if file_len > MAX_RESIDENT_KWVIDEO_BYTES {
            return Err(KwVideoError::ResidentVideoTooLarge {
                required: file_len,
                maximum: MAX_RESIDENT_KWVIDEO_BYTES,
            });
        }

        // Keep the active compressed MP4 resident. FAT/eMMC currently reads one
        // 512-byte sector per transaction; doing that at each one-second fragment
        // boundary blocks CPU1 long enough to miss a vblank even with a full
        // decoded-frame queue. Pay the storage cost once before playback starts.
        let mut resident = vec![0u8; file_len];
        resident[..resume_offset].copy_from_slice(&bootstrap);
        vfs.read_exact_stream(&mut stream, &mut resident[resume_offset..])?;

        Ok(Self {
            file_type,
            metadata,
            stream,
            bootstrap: bootstrap.into_boxed_slice(),
            resident: resident.into_boxed_slice(),
            resident_position: resume_offset,
            first_fragment_offset,
            resume_offset,
        })
    }

    #[inline(always)]
    pub const fn file_type(&self) -> FileType {
        self.file_type
    }

    #[inline(always)]
    pub const fn metadata(&self) -> VideoMetadata {
        self.metadata
    }

    #[inline(always)]
    pub fn stream_name(&self) -> &str {
        self.stream.name()
    }

    #[inline(always)]
    pub fn file_len(&self) -> usize {
        self.stream.len()
    }

    #[inline(always)]
    pub fn resident_len(&self) -> usize {
        self.resident.len()
    }

    /// Cached bytes from file offset zero through the first `mdat`.
    #[inline(always)]
    pub fn bootstrap(&self) -> &[u8] {
        &self.bootstrap
    }

    pub fn first_fragment_samples(&self) -> Result<Vec<FragmentSample>, KwVideoError> {
        parse_first_fragment_samples(
            &self.bootstrap,
            self.first_fragment_offset,
            self.metadata.track_id,
        )
    }

    pub fn sample_bytes(&self, sample: FragmentSample) -> Result<&[u8], KwVideoError> {
        let end = sample
            .offset
            .checked_add(sample.size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
        self.bootstrap
            .get(sample.offset..end)
            .ok_or(KwVideoError::InvalidStructure(
                "HEVC sample escapes transition bootstrap",
            ))
    }

    pub fn picture_syntax(
        &self,
        sample: FragmentSample,
    ) -> Result<super::hevc_syntax::FirstPictureSyntax, KwVideoError> {
        self.picture_syntax_bytes(self.sample_bytes(sample)?)
    }

    pub fn picture_syntax_bytes(
        &self,
        sample: &[u8],
    ) -> Result<super::hevc_syntax::FirstPictureSyntax, KwVideoError> {
        let hvcc = find_hvcc_in_bootstrap(&self.bootstrap)?;
        super::hevc_syntax::FirstPictureSyntax::parse(
            hvcc.payload,
            sample,
            self.metadata.hevc.nal_length_size as usize,
        )
        .map_err(KwVideoError::HevcSyntax)
    }

    pub fn picture_cabac_payload<'a>(
        &'a self,
        sample: FragmentSample,
        picture: &super::hevc_syntax::FirstPictureSyntax,
    ) -> Result<&'a [u8], KwVideoError> {
        self.picture_cabac_payload_bytes(self.sample_bytes(sample)?, picture)
    }

    pub fn picture_cabac_payload_bytes<'a>(
        &self,
        sample: &'a [u8],
        picture: &super::hevc_syntax::FirstPictureSyntax,
    ) -> Result<&'a [u8], KwVideoError> {
        let nal_start = picture.slice_nal_sample_offset;
        let nal_end = nal_start
            .checked_add(picture.slice_nal_len)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let payload_start = nal_start
            .checked_add(picture.slice.data_byte_offset)
            .ok_or(KwVideoError::OffsetOverflow)?;
        if payload_start >= nal_end {
            return Err(KwVideoError::InvalidStructure(
                "HEVC slice has no CABAC payload",
            ));
        }
        sample
            .get(payload_start..nal_end)
            .ok_or(KwVideoError::InvalidStructure(
                "HEVC CABAC payload escapes sample",
            ))
    }

    pub fn picture_cabac_boundary_has_epb_bytes(
        &self,
        sample: &[u8],
        picture: &super::hevc_syntax::FirstPictureSyntax,
    ) -> Result<bool, KwVideoError> {
        let nal_start = picture.slice_nal_sample_offset;
        let nal_end = nal_start
            .checked_add(picture.slice_nal_len)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let payload_start = nal_start
            .checked_add(picture.slice.data_byte_offset)
            .ok_or(KwVideoError::OffsetOverflow)?;
        if payload_start >= nal_end {
            return Err(KwVideoError::InvalidStructure(
                "HEVC slice has no CABAC payload",
            ));
        }
        Ok(payload_start >= nal_start + 2
            && sample.get(payload_start) == Some(&0x03)
            && sample.get(payload_start - 1) == Some(&0x00)
            && sample.get(payload_start - 2) == Some(&0x00))
    }

    /// Return the CABAC slice data as RBSP bytes.
    ///
    /// RPIVID can remove emulation-prevention bytes itself, but our stateless
    /// path starts the hardware at the first CABAC byte rather than at the NAL
    /// header. An emulation-prevention byte may have been inserted across the
    /// slice-header/CABAC boundary; starting after the header loses the two
    /// preceding zero bytes that make that 0x03 recognizable. De-escape the
    /// complete NAL payload first, then cut at the parsed RBSP header boundary.
    pub fn picture_cabac_rbsp_bytes(
        &self,
        sample: &[u8],
        picture: &super::hevc_syntax::FirstPictureSyntax,
    ) -> Result<Vec<u8>, KwVideoError> {
        let nal_start = picture.slice_nal_sample_offset;
        let nal_end = nal_start
            .checked_add(picture.slice_nal_len)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let nal = sample
            .get(nal_start..nal_end)
            .ok_or(KwVideoError::InvalidStructure(
                "HEVC slice NAL escapes sample",
            ))?;
        if nal.len() < 2 {
            return Err(KwVideoError::InvalidStructure(
                "HEVC slice NAL is shorter than its header",
            ));
        }

        let mut rbsp = Vec::with_capacity(nal.len() - 2);
        let mut zero_run = 0u8;
        for &byte in &nal[2..] {
            if zero_run >= 2 && byte == 0x03 {
                zero_run = 0;
                continue;
            }
            rbsp.push(byte);
            if byte == 0 {
                zero_run = zero_run.saturating_add(1);
            } else {
                zero_run = 0;
            }
        }

        let header_bytes = picture.slice.header_rbsp_bits / 8;
        if picture.slice.header_rbsp_bits & 7 != 0 || header_bytes >= rbsp.len() {
            return Err(KwVideoError::InvalidStructure(
                "HEVC slice has no RBSP CABAC payload",
            ));
        }
        Ok(rbsp[header_bytes..].to_vec())
    }

    /// Rebuild the first `hvc1` sample as one Annex-B access unit.
    ///
    /// MP4 stores VPS/SPS/PPS in `hvcC` and the sample itself as 4-byte
    /// length-prefixed NAL units. The decoder bring-up path consumes one
    /// contiguous byte window, so prepend the out-of-band parameter sets and
    /// replace every sample length prefix with a four-byte Annex-B start code.
    /// Parse the first IDR picture into the stateless HEVC fields RPIVID needs.
    pub fn first_picture_syntax(
        &self,
    ) -> Result<super::hevc_syntax::FirstPictureSyntax, KwVideoError> {
        let fragment = self.metadata.first_fragment;
        let sample_start = fragment.first_sample_offset;
        let sample_end = sample_start
            .checked_add(fragment.first_sample_size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let sample = self
            .bootstrap
            .get(sample_start..sample_end)
            .ok_or(KwVideoError::InvalidStructure(
                "first HEVC sample escapes transition bootstrap",
            ))?;
        let hvcc = find_hvcc_in_bootstrap(&self.bootstrap)?;

        super::hevc_syntax::FirstPictureSyntax::parse(
            hvcc.payload,
            sample,
            self.metadata.hevc.nal_length_size as usize,
        )
        .map_err(KwVideoError::HevcSyntax)
    }

    pub fn first_picture_cabac_payload<'a>(
        &'a self,
        picture: &super::hevc_syntax::FirstPictureSyntax,
    ) -> Result<&'a [u8], KwVideoError> {
        let fragment = self.metadata.first_fragment;
        let sample_start = fragment.first_sample_offset;
        let nal_start = sample_start
            .checked_add(picture.slice_nal_sample_offset)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let nal_end = nal_start
            .checked_add(picture.slice_nal_len)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let payload_start = nal_start
            .checked_add(picture.slice.data_byte_offset)
            .ok_or(KwVideoError::OffsetOverflow)?;

        if payload_start >= nal_end {
            return Err(KwVideoError::InvalidStructure(
                "first HEVC slice has no CABAC payload",
            ));
        }

        self.bootstrap
            .get(payload_start..nal_end)
            .ok_or(KwVideoError::InvalidStructure(
                "first HEVC CABAC payload escapes transition bootstrap",
            ))
    }

    pub fn first_access_unit_annex_b(&self) -> Result<FirstAccessUnit, KwVideoError> {
        let fragment = self.metadata.first_fragment;
        let sample_start = fragment.first_sample_offset;
        let sample_end = sample_start
            .checked_add(fragment.first_sample_size as usize)
            .ok_or(KwVideoError::OffsetOverflow)?;
        let sample = self
            .bootstrap
            .get(sample_start..sample_end)
            .ok_or(KwVideoError::InvalidStructure(
                "first HEVC sample escapes transition bootstrap",
            ))?;

        let hvcc = find_hvcc_in_bootstrap(&self.bootstrap)?;
        let mut bytes = Vec::with_capacity(
            sample
                .len()
                .checked_add(2048)
                .ok_or(KwVideoError::OffsetOverflow)?,
        );
        let parameter_set_count = append_parameter_sets(hvcc.payload, &mut bytes)?;
        let (sample_nal_count, vcl_nal_count, first_vcl_type) = append_length_prefixed_sample(
            sample,
            self.metadata.hevc.nal_length_size as usize,
            &mut bytes,
        )?;

        Ok(FirstAccessUnit {
            bytes: bytes.into_boxed_slice(),
            parameter_set_count,
            sample_nal_count,
            vcl_nal_count,
            first_vcl_type,
        })
    }

    /// Copy the cached transition fragment into an owned fragment buffer.
    ///
    /// This is used by the bring-up playback loop so cached and streamed
    /// fragments share the same parsing path. The permanently resident copy
    /// remains in `bootstrap`; only the compressed media bytes are duplicated.
    pub fn cached_first_fragment(&self) -> Result<MediaFragment, KwVideoError> {
        let bytes = self
            .bootstrap
            .get(self.first_fragment_offset..self.resume_offset)
            .ok_or(KwVideoError::InvalidStructure(
                "cached first fragment escapes transition bootstrap",
            ))?
            .to_vec()
            .into_boxed_slice();
        parse_media_fragment(bytes, self.first_fragment_offset, self.metadata.track_id)
    }

    /// Read the next `moof`/`mdat` pair from eMMC.
    ///
    /// Unknown top-level boxes before the next fragment are skipped. The
    /// returned compressed fragment is owned so decoding can proceed without
    /// keeping a mutable borrow of the VFS stream.
    pub fn read_next_fragment(
        &mut self,
        _vfs: &Vfs<'_>,
    ) -> Result<Option<MediaFragment>, KwVideoError> {
        let bytes = &self.resident;
        let mut cursor = self.resident_position;

        loop {
            if cursor == bytes.len() {
                self.resident_position = cursor;
                return Ok(None);
            }
            if cursor > bytes.len() {
                return Err(KwVideoError::InvalidStructure(
                    "resident KWVIDEO cursor escaped file",
                ));
            }

            let remaining = &bytes[cursor..];
            let header = super::isobmff::BoxHeader::parse(remaining, remaining.len())?;
            let box_start = cursor;
            let box_end = box_start
                .checked_add(header.box_size())
                .ok_or(KwVideoError::OffsetOverflow)?;

            if header.box_type() != MOOF_BOX {
                cursor = box_end;
                continue;
            }

            if box_end >= bytes.len() {
                return Err(KwVideoError::MissingFirstMediaFragment);
            }

            let mdat_remaining = &bytes[box_end..];
            let mdat = super::isobmff::BoxHeader::parse(mdat_remaining, mdat_remaining.len())?;
            if mdat.box_type() != MDAT_BOX {
                return Err(KwVideoError::InvalidStructure(
                    "kwv1 moof is not immediately followed by mdat",
                ));
            }
            let fragment_end = box_end
                .checked_add(mdat.box_size())
                .ok_or(KwVideoError::OffsetOverflow)?;
            let fragment_bytes = bytes
                .get(box_start..fragment_end)
                .ok_or(KwVideoError::InvalidStructure(
                    "resident KWVIDEO fragment escapes file",
                ))?
                .to_vec()
                .into_boxed_slice();

            self.resident_position = fragment_end;
            return Ok(Some(parse_media_fragment(
                fragment_bytes,
                box_start,
                self.metadata.track_id,
            )?));
        }
    }

    /// Offset of the first cached `moof` within `bootstrap()`.
    #[inline(always)]
    pub const fn first_fragment_offset(&self) -> usize {
        self.first_fragment_offset
    }

    /// File offset immediately after the cached first `mdat`.
    #[inline(always)]
    pub const fn resume_offset(&self) -> usize {
        self.resume_offset
    }

    /// Stream the uncached remainder of the active video.
    #[inline(always)]
    pub fn stream_mut(&mut self) -> &mut FileStream {
        &mut self.stream
    }

    /// Return the eMMC stream to the first uncached fragment.
    ///
    /// Profile re-activation and video looping can replay `bootstrap()` from
    /// RAM and reset this stream without rereading the cached bytes from disk.
    pub fn rewind_stream(&mut self, _vfs: &Vfs<'_>) -> Result<(), KwVideoError> {
        self.resident_position = self.resume_offset;
        Ok(())
    }
}

/// Resident transition state for every profile video.
///
/// Slots 0..=7 are available to the normal profiles. Slot 8 is reserved for
/// the optional fallback visual profile; input behavior for that slot remains
/// governed by the keyboard fallback layout rather than by this video cache.
pub struct VideoTransitionCache {
    slots: [Option<VideoAsset>; VIDEO_PROFILE_SLOT_COUNT],
}

impl VideoTransitionCache {
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
        }
    }

    pub fn load(
        &mut self,
        vfs: &Vfs<'_>,
        slot: usize,
        fs: FsId,
        path: &str,
    ) -> Result<(), KwVideoError> {
        let destination = self
            .slots
            .get_mut(slot)
            .ok_or(KwVideoError::VideoProfileSlotOutOfRange(slot))?;
        *destination = Some(VideoAsset::open(vfs, fs, path)?);
        Ok(())
    }

    pub fn unload(&mut self, slot: usize) -> Result<Option<VideoAsset>, KwVideoError> {
        let asset = self
            .slots
            .get_mut(slot)
            .ok_or(KwVideoError::VideoProfileSlotOutOfRange(slot))?;
        Ok(asset.take())
    }

    #[inline(always)]
    pub fn get(&self, slot: usize) -> Option<&VideoAsset> {
        self.slots.get(slot).and_then(Option::as_ref)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, slot: usize) -> Option<&mut VideoAsset> {
        self.slots.get_mut(slot).and_then(Option::as_mut)
    }

    pub fn loaded_count(&self) -> usize {
        self.slots.iter().filter(|asset| asset.is_some()).count()
    }

    pub fn bootstrap_bytes(&self) -> usize {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|asset| asset.bootstrap().len())
            .sum()
    }
}

impl Default for VideoTransitionCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum KwVideoError {
    EmptyFile,
    MissingFileType(FourCc),
    InvalidFileTypeSize(usize),
    MisalignedFileTypeSize(usize),
    UnsupportedMajorBrand(FourCc),
    FragmentBeforeMovieBox,
    MovieBoxAfterFragment,
    MediaDataBeforeFragment,
    MissingFirstMediaFragment,
    TransitionBootstrapTooLarge {
        required: usize,
        maximum: usize,
    },
    ResidentVideoTooLarge {
        required: usize,
        maximum: usize,
    },
    OffsetOverflow,
    InvalidStructure(&'static str),
    UnsupportedProfile(&'static str),
    VideoProfileSlotOutOfRange(usize),
    IsoBmff(IsoBmffError),
    HevcSyntax(super::hevc_syntax::HevcSyntaxError),
}

impl From<IsoBmffError> for KwVideoError {
    fn from(error: IsoBmffError) -> Self {
        Self::IsoBmff(error)
    }
}

impl From<VfsError> for KwVideoError {
    fn from(error: VfsError) -> Self {
        Self::IsoBmff(IsoBmffError::Vfs(error))
    }
}

impl fmt::Display for KwVideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFile => write!(f, "empty Keywork Video file"),
            Self::MissingFileType(found) => {
                write!(f, "Keywork Video must begin with ftyp, found {found}")
            }
            Self::InvalidFileTypeSize(size) => {
                write!(f, "Keywork Video ftyp payload is too small: {size} bytes")
            }
            Self::MisalignedFileTypeSize(size) => {
                write!(f, "Keywork Video ftyp payload is not a multiple of four: {size} bytes")
            }
            Self::UnsupportedMajorBrand(brand) => {
                write!(f, "unsupported video major brand {brand}; expected {KWVIDEO_BRAND}")
            }
            Self::FragmentBeforeMovieBox => {
                write!(f, "Keywork Video media fragment appears before moov")
            }
            Self::MovieBoxAfterFragment => {
                write!(f, "Keywork Video moov appears after the first media fragment")
            }
            Self::MediaDataBeforeFragment => {
                write!(f, "Keywork Video mdat appears before the first moof")
            }
            Self::MissingFirstMediaFragment => {
                write!(f, "Keywork Video has no complete first moof/mdat fragment")
            }
            Self::TransitionBootstrapTooLarge { required, maximum } => {
                write!(
                    f,
                    "Keywork Video transition bootstrap requires {required} bytes, maximum is {maximum}"
                )
            }
            Self::ResidentVideoTooLarge { required, maximum } => {
                write!(
                    f,
                    "Keywork Video resident playback requires {required} bytes, maximum is {maximum}"
                )
            }
            Self::OffsetOverflow => write!(f, "Keywork Video box offset overflow"),
            Self::InvalidStructure(reason) => write!(f, "invalid Keywork Video structure: {reason}"),
            Self::UnsupportedProfile(reason) => write!(f, "unsupported kwv1 profile: {reason}"),
            Self::VideoProfileSlotOutOfRange(slot) => write!(
                f,
                "video profile slot {slot} is out of range; maximum is {}",
                VIDEO_PROFILE_SLOT_COUNT - 1
            ),
            Self::IsoBmff(error) => fmt::Display::fmt(error, f),
            Self::HevcSyntax(error) => fmt::Display::fmt(error, f),
        }
    }
}
