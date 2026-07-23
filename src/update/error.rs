use crate::storage::fs::vfs::VfsError;

#[derive(Debug)]
pub enum UpdateError {
    Vfs(VfsError),

    InvalidImage,
    VerifyFailed,
    MissingStagedImage,

    Alloc,
    NoActiveSession,
    BadSequence,
    TooMuchData,
    BadPacket,
    BadMagic,
    BadCommand,
    CrcFailed,
    UnexpectedEnd,
}

impl From<VfsError> for UpdateError {
    fn from(e: VfsError) -> Self {
        Self::Vfs(e)
    }
}
