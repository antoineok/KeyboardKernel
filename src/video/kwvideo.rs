//! Keywork Video (`kwv1`) profile validation.
//!
//! A Keywork Video file is still ordinary ISO-BMFF/MP4. The custom `kwv1`
//! major brand only promises that the file follows the constrained subset the
//! kernel supports.

use core::fmt;

use alloc::boxed::Box;
use alloc::vec;

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


/// A `kwv1` asset prepared for instant profile transitions.
///
/// The complete video stays on FAT/eMMC. Only the ISO-BMFF initialization
/// segment and first media fragment are pinned in RAM. Because the host muxer
/// starts every fragment with a keyframe, that cached fragment is sufficient to
/// begin decoding immediately while the file stream is already positioned at
/// the next fragment.
pub struct VideoAsset {
    file_type: FileType,
    stream: FileStream,
    bootstrap: Box<[u8]>,
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

        Ok(Self {
            file_type,
            stream,
            bootstrap: bootstrap.into_boxed_slice(),
            first_fragment_offset,
            resume_offset,
        })
    }

    #[inline(always)]
    pub const fn file_type(&self) -> FileType {
        self.file_type
    }

    /// Cached bytes from file offset zero through the first `mdat`.
    #[inline(always)]
    pub fn bootstrap(&self) -> &[u8] {
        &self.bootstrap
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
    pub fn rewind_stream(&mut self, vfs: &Vfs<'_>) -> Result<(), KwVideoError> {
        vfs.seek_stream(&mut self.stream, self.resume_offset)?;
        Ok(())
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
    OffsetOverflow,
    IsoBmff(IsoBmffError),
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
            Self::OffsetOverflow => write!(f, "Keywork Video box offset overflow"),
            Self::IsoBmff(error) => fmt::Display::fmt(error, f),
        }
    }
}
