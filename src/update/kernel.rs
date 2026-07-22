use crate::{
    drivers::{uart::UART, watchdog::WATCHDOG},
    storage::fs::vfs::{FsId, VFS},
};

use super::{sha256::sha256, UpdateError, UPDATE_MAGIC};

const KERNEL_CURRENT: &str = "kernel8.img";
const KERNEL_NEW: &str = "kernel8.new";
const KERNEL_OLD: &str = "kernel8.old";
const UPDATE_FS: FsId = FsId::Boot;

pub struct KernelUpdater;

impl KernelUpdater {
    pub fn stage(image: &[u8]) -> Result<(), UpdateError> {
        if image.len() < 4096 {
            return Err(UpdateError::InvalidImage);
        }

        Self::verify_magic_bytes(image)?;

        let vfs = VFS.get_mut();
        if vfs.exists_fs(UPDATE_FS, KERNEL_NEW) {
            vfs.delete_fs(UPDATE_FS, KERNEL_NEW)?;
        }
        vfs.write_fs(UPDATE_FS, KERNEL_NEW, image)?;
        Ok(())
    }

    pub fn verify_staged(expected_size: usize, expected_sha256: &[u8; 32]) -> Result<(), UpdateError> {
        let vfs = VFS.get_mut();
        let file = vfs
            .open_fs(UPDATE_FS, KERNEL_NEW)
            .map_err(|_| UpdateError::MissingStagedImage)?;

        if file.len() != expected_size {
            return Err(UpdateError::VerifyFailed);
        }
        Self::verify_magic_bytes(file.as_bytes())?;
        if &sha256(file.as_bytes()) != expected_sha256 {
            return Err(UpdateError::VerifyFailed);
        }
        Ok(())
    }

    pub fn discard_staged() -> Result<(), UpdateError> {
        let vfs = VFS.get_mut();
        if vfs.exists_fs(UPDATE_FS, KERNEL_NEW) {
            vfs.delete_fs(UPDATE_FS, KERNEL_NEW)?;
        }
        Ok(())
    }

    pub fn has_staged() -> bool {
        VFS.get().exists_fs(UPDATE_FS, KERNEL_NEW)
    }

    pub fn commit_and_reboot() -> ! {
        let vfs = VFS.get_mut();

        if vfs.exists_fs(UPDATE_FS, KERNEL_OLD) {
            vfs.delete_fs(UPDATE_FS, KERNEL_OLD).unwrap();
        }

        vfs.rename_fs(UPDATE_FS, KERNEL_CURRENT, KERNEL_OLD).unwrap();
        vfs.rename_fs(UPDATE_FS, KERNEL_NEW, KERNEL_CURRENT).unwrap();

        UART.get().write_text("FIRMWARE COMMIT DONE, REBOOTING\n");
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
        WATCHDOG.get().reboot();
    }

    fn verify_magic_bytes(image: &[u8]) -> Result<(), UpdateError> {
        if image.len() < 4 {
            return Err(UpdateError::InvalidImage);
        }

        let offset = image.len() - 4;
        let magic = u32::from_le_bytes([
            image[offset],
            image[offset + 1],
            image[offset + 2],
            image[offset + 3],
        ]);

        if magic != UPDATE_MAGIC {
            return Err(UpdateError::BadMagic);
        }
        Ok(())
    }
}
