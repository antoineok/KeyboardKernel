extern crate alloc;

use core::cell::UnsafeCell;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::reader::Reader;
use crate::storage::block::{BlockDevice, BlockError};
use crate::storage::fs::fat32::file::{FatFile, FatFileReader};
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

/// Files opened through `open_fs` are intentionally limited because that API
/// materializes the complete file in the general-purpose kernel heap.
pub const MAX_EAGER_FILE_SIZE: usize = 16 * 1024 * 1024;

//--------------------------------------------------------------------------------------------------
// Drive IDs
//--------------------------------------------------------------------------------------------------
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FsId {
    Boot,
    Rom,
}

#[derive(Debug)]
pub enum VfsError {
    UnknownScheme,
    NotMounted(FsId),
    InvalidPath(&'static str),
    Fat(Fat32Error),
    Block(BlockError),
    Utf8,
    FileTooLargeForEagerRead { size: usize, maximum: usize },
    UnexpectedEof { expected: usize, actual: usize },
}

impl From<Fat32Error> for VfsError {
    fn from(e: Fat32Error) -> Self {
        VfsError::Fat(e)
    }
}

impl From<BlockError> for VfsError {
    fn from(e: BlockError) -> Self {
        VfsError::Block(e)
    }
}

//--------------------------------------------------------------------------------------------------
// Eager file handle (owns bytes, can create Reader view safely)
//--------------------------------------------------------------------------------------------------
pub struct FileHandle {
    name: String,
    data: Vec<u8>,
}

impl FileHandle {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Safe Reader view tied to self lifetime (no leaks).
    pub fn reader(&self) -> Reader {
        Reader::from_slice(&self.data)
    }

    /// Optional helper for text files.
    pub fn as_str(&self) -> Result<&str, VfsError> {
        core::str::from_utf8(&self.data).map_err(|_| VfsError::Utf8)
    }
}

//--------------------------------------------------------------------------------------------------
// Streaming file handle
//--------------------------------------------------------------------------------------------------
pub struct FileStream {
    fs: FsId,
    reader: FatFileReader,
}

impl FileStream {
    pub fn name(&self) -> &str {
        self.reader.name()
    }

    pub fn len(&self) -> usize {
        self.reader.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reader.is_empty()
    }

    pub fn position(&self) -> usize {
        self.reader.position()
    }

    pub fn remaining(&self) -> usize {
        self.reader.remaining()
    }
}

//--------------------------------------------------------------------------------------------------
// Mounts
//--------------------------------------------------------------------------------------------------
struct FatMount<'a> {
    dev: &'a dyn BlockDevice,
    vol: Fat32Volume,
}

pub struct Vfs<'a> {
    boot: Option<FatMount<'a>>,
    rom: Option<FatMount<'a>>,
}

impl<'a> Vfs<'a> {
    pub const fn new() -> Self {
        Self {
            boot: None,
            rom: None,
        }
    }

    /// Mount a FAT32 volume for a given FsId.
    /// `dev` must be partition-relative (sector 0 is the VBR).
    pub fn mount_fat32(&mut self, fs: FsId, dev: &'a dyn BlockDevice) -> Result<(), VfsError> {
        let vol = Fat32Volume::mount(dev)?;
        match fs {
            FsId::Boot => self.boot = Some(FatMount { dev, vol }),
            FsId::Rom => self.rom = Some(FatMount { dev, vol }),
        }
        Ok(())
    }

    fn get_mount(&self, fs: FsId) -> Result<&FatMount<'a>, VfsError> {
        match fs {
            FsId::Boot => self.boot.as_ref().ok_or(VfsError::NotMounted(FsId::Boot)),
            FsId::Rom => self.rom.as_ref().ok_or(VfsError::NotMounted(FsId::Rom)),
        }
    }

    fn get_mount_mut(&mut self, fs: FsId) -> Result<&mut FatMount<'a>, VfsError> {
        match fs {
            FsId::Boot => self.boot.as_mut().ok_or(VfsError::NotMounted(FsId::Boot)),
            FsId::Rom => self.rom.as_mut().ok_or(VfsError::NotMounted(FsId::Rom)),
        }
    }

    //----------------------------------------------------------------------------------------------
    // Open APIs
    //----------------------------------------------------------------------------------------------

    pub fn exists_fs(&self, fs: FsId, path: &str) -> bool {
        let path = match normalize_path(path) {
            Ok(p) => p,
            Err(_) => return false,
        };

        let m = match self.get_mount(fs) {
            Ok(m) => m,
            Err(_) => return false,
        };

        m.vol.exists(m.dev, &path)
    }

    pub fn exists_uri(&self, uri: &str) -> bool {
        let (fs, path) = match parse_uri(uri) {
            Ok(v) => v,
            Err(_) => return false,
        };

        self.exists_fs(fs, path)
    }

    /// Open a small file by `(drive, path)` and read it completely into memory.
    ///
    /// Large assets must use `open_stream_fs` so a profile cannot exhaust the
    /// general-purpose heap merely by opening one file.
    pub fn open_fs(&self, fs: FsId, path: &str) -> Result<FileHandle, VfsError> {
        let path = normalize_path(path)?;
        let m = self.get_mount(fs)?;
        let file: FatFile = m.vol.open_file_path(m.dev, &path)?;
        let size = file.size as usize;

        if size > MAX_EAGER_FILE_SIZE {
            return Err(VfsError::FileTooLargeForEagerRead {
                size,
                maximum: MAX_EAGER_FILE_SIZE,
            });
        }

        let mut data = vec![0u8; size];
        let read = m.vol.read_file_prefix(m.dev, &file, &mut data)?;

        if read != size {
            return Err(VfsError::UnexpectedEof {
                expected: size,
                actual: read,
            });
        }

        Ok(FileHandle {
            name: file.name,
            data,
        })
    }

    /// Open a small file by URI and read it completely into memory.
    pub fn open_uri(&self, uri: &str) -> Result<FileHandle, VfsError> {
        let (fs, path) = parse_uri(uri)?;
        self.open_fs(fs, path)
    }

    /// Open a file without allocating storage for its contents.
    pub fn open_stream_fs(&self, fs: FsId, path: &str) -> Result<FileStream, VfsError> {
        let path = normalize_path(path)?;
        let m = self.get_mount(fs)?;
        let file = m.vol.open_file_path(m.dev, &path)?;
        let reader = m.vol.open_file_reader(file)?;

        Ok(FileStream { fs, reader })
    }

    /// Open a streaming file by URI.
    pub fn open_stream_uri(&self, uri: &str) -> Result<FileStream, VfsError> {
        let (fs, path) = parse_uri(uri)?;
        self.open_stream_fs(fs, path)
    }

    /// Read the next bytes from a streaming file.
    pub fn read_stream(
        &self,
        stream: &mut FileStream,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let mount = self.get_mount(stream.fs)?;
        Ok(mount.vol.read_file(mount.dev, &mut stream.reader, buffer)?)
    }

    /// Fill `buffer` from a streaming file or report a short file.
    pub fn read_exact_stream(
        &self,
        stream: &mut FileStream,
        mut buffer: &mut [u8],
    ) -> Result<(), VfsError> {
        let expected = buffer.len();
        let mut total = 0usize;

        while !buffer.is_empty() {
            let read = self.read_stream(stream, buffer)?;

            if read == 0 {
                return Err(VfsError::UnexpectedEof {
                    expected,
                    actual: total,
                });
            }

            total += read;
            buffer = &mut buffer[read..];
        }

        Ok(())
    }

    /// Advance a streaming file without allocating a temporary buffer.
    pub fn skip_stream(&self, stream: &mut FileStream, mut amount: usize) -> Result<(), VfsError> {
        let expected = stream.position().checked_add(amount).unwrap_or(usize::MAX);
        let mut scratch = [0u8; 512];

        while amount != 0 {
            let chunk = core::cmp::min(amount, scratch.len());
            let read = self.read_stream(stream, &mut scratch[..chunk])?;

            if read == 0 {
                return Err(VfsError::UnexpectedEof {
                    expected,
                    actual: stream.position(),
                });
            }

            amount -= read;
        }

        Ok(())
    }

    pub fn write_fs(&mut self, fs: FsId, path: &str, data: &[u8]) -> Result<(), VfsError> {
        let path = normalize_path(path)?;
        let m = self.get_mount_mut(fs)?;

        let file = match m.vol.open_file_path(m.dev, &path) {
            Ok(f) => f,
            Err(_) => m.vol.create_file_83(m.dev, &path)?,
        };

        m.vol.write_file_all_growing(m.dev, &file, data)?;

        Ok(())
    }

    pub fn write_uri(&mut self, uri: &str, data: &[u8]) -> Result<(), VfsError> {
        let (fs, path) = parse_uri(uri)?;
        self.write_fs(fs, path, data)
    }

    pub fn rename_fs(&self, fs: FsId, old_path: &str, new_path: &str) -> Result<(), VfsError> {
        let old_path = normalize_path(old_path)?;
        let new_path = normalize_path(new_path)?;
        let m = self.get_mount(fs)?;
        m.vol.rename_file_83(m.dev, &old_path, &new_path)?;
        Ok(())
    }

    pub fn rename_uri(&self, old_uri: &str, new_uri: &str) -> Result<(), VfsError> {
        let (old_fs, old_path) = parse_uri(old_uri)?;
        let (new_fs, new_path) = parse_uri(new_uri)?;

        if old_fs != new_fs {
            return Err(VfsError::InvalidPath(
                "cross-filesystem rename not supported",
            ));
        }

        self.rename_fs(old_fs, old_path, new_path)
    }

    pub fn delete_fs(&mut self, fs: FsId, path: &str) -> Result<(), VfsError> {
        let path = normalize_path(path)?;
        let m = self.get_mount_mut(fs)?;
        m.vol.delete_file_83(m.dev, &path)?;
        Ok(())
    }

    pub fn delete_uri(&mut self, uri: &str) -> Result<(), VfsError> {
        let (fs, path) = parse_uri(uri)?;
        self.delete_fs(fs, path)
    }
}

//--------------------------------------------------------------------------------------------------
// Helpers: URI + path normalization
//--------------------------------------------------------------------------------------------------

fn parse_uri(uri: &str) -> Result<(FsId, &str), VfsError> {
    let (scheme, rest) = uri
        .split_once(':')
        .ok_or(VfsError::InvalidPath("missing ':' in uri"))?;

    let fs = match scheme.to_ascii_lowercase().as_str() {
        "bootfs" => FsId::Boot,
        "romfs" => FsId::Rom,
        _ => return Err(VfsError::UnknownScheme),
    };

    let mut path = rest;

    if let Some(stripped) = path.strip_prefix("//") {
        path = stripped;
    }
    if let Some(stripped) = path.strip_prefix('/') {
        path = stripped;
    }

    Ok((fs, path))
}

/// Normalize a root-relative path:
/// - accept `/a/b` or `a/b`
/// - collapse repeated `/`
/// - reject `..`
/// - remove the leading `/`
fn normalize_path(path: &str) -> Result<String, VfsError> {
    let mut path = path;

    while let Some(rest) = path.strip_prefix('/') {
        path = rest;
    }

    if path.is_empty() {
        return Err(VfsError::InvalidPath("empty path"));
    }

    let mut output = String::new();
    let mut first = true;

    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(VfsError::InvalidPath(".. not supported"));
        }
        if !first {
            output.push('/');
        }
        output.push_str(component);
        first = false;
    }

    if output.is_empty() {
        return Err(VfsError::InvalidPath("path reduced to empty"));
    }

    Ok(output)
}

//--------------------------------------------------------------------------------------------------

pub struct VfsStatic(UnsafeCell<Vfs<'static>>);

unsafe impl Sync for VfsStatic {}

pub static VFS: VfsStatic = VfsStatic(UnsafeCell::new(Vfs::new()));

impl VfsStatic {
    pub fn get(&self) -> &Vfs<'static> {
        unsafe { &*self.0.get() }
    }

    pub fn get_mut(&self) -> &mut Vfs<'static> {
        unsafe { &mut *self.0.get() }
    }
}
