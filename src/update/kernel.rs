use crate::{
    drivers::watchdog::WATCHDOG, info, storage::fs::vfs::{FsId, VFS}
};

use super::error::UpdateError;

const KERNEL_CURRENT: &str = "kernel8.img";
const KERNEL_NEW: &str = "kernel8.new";
const KERNEL_OLD: &str = "kernel8.old";

pub struct KernelUpdater;

impl KernelUpdater {
    pub fn stage(image: &[u8]) -> Result<(), UpdateError> {
        if image.is_empty() {
            return Err(UpdateError::InvalidImage);
        }

        // optional very basic sanity check
        if image.len() < 4096 {
            return Err(UpdateError::InvalidImage);
        }

        let vfs = VFS.get_mut();

        if vfs.exists_fs(FsId::Rom, KERNEL_NEW) {
            vfs.delete_fs(FsId::Rom, KERNEL_NEW)?;
        }

        vfs.write_fs(FsId::Rom, KERNEL_NEW, image)?;

        Ok(())
    }

    pub fn verify_staged(expected_size: usize) -> Result<(), UpdateError> {
        let vfs = VFS.get_mut();

        let file = vfs.open_fs(FsId::Rom, KERNEL_NEW)
            .map_err(|_| UpdateError::MissingStagedImage)?;

        info!(
            "verify_staged: expected_size={}, actual_size={}",
            expected_size,
            file.len()
        );

        if file.len() != expected_size {
            return Err(UpdateError::VerifyFailed);
        }

        Ok(())
    }

    pub fn commit_and_reboot() -> ! {
        let vfs = VFS.get_mut();

        if vfs.exists_fs(FsId::Rom, KERNEL_OLD) {
            vfs.delete_fs(FsId::Rom, KERNEL_OLD).unwrap();
        }

        vfs.rename_fs(FsId::Rom, KERNEL_CURRENT, KERNEL_OLD).unwrap();
        vfs.rename_fs(FsId::Rom, KERNEL_NEW, KERNEL_CURRENT).unwrap();

        WATCHDOG.get().reboot();
    }
}