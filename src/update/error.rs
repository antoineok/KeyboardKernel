use crate::storage::fs::vfs::VfsError;

#[derive(Debug)]
pub enum UpdateError {
    Vfs(VfsError),

    InvalidImage,
    VerifyFailed,
    MissingStagedImage,

    BadMagic,
}

impl From<VfsError> for UpdateError {
    fn from(e: VfsError) -> Self {
        Self::Vfs(e)
    }
}
