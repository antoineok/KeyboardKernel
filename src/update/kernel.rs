use crate::{
    drivers::{uart::UART, watchdog::WATCHDOG},
    storage::fs::vfs::{FsId, VFS},
};

use super::{UpdateError, UPDATE_MAGIC};

const KERNEL_CURRENT: &str = "kernel8.img";
const KERNEL_NEW: &str = "kernel8.new";
const KERNEL_OLD: &str = "kernel8.old";

// TESTING TARGET.
// Later change to FsId::Boot.
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

    pub fn verify_staged(expected_size: usize) -> Result<(), UpdateError> {
        let vfs = VFS.get_mut();

        let file = vfs
            .open_fs(UPDATE_FS, KERNEL_NEW)
            .map_err(|_| UpdateError::MissingStagedImage)?;

        if file.len() != expected_size {
            return Err(UpdateError::VerifyFailed);
        }

        Self::verify_magic_bytes(file.as_bytes())?;

        Ok(())
    }

    pub fn commit_and_reboot() -> ! {
        let vfs = VFS.get_mut();

        if vfs.exists_fs(UPDATE_FS, KERNEL_OLD) {
            vfs.delete_fs(UPDATE_FS, KERNEL_OLD).unwrap();
        }

        vfs.rename_fs(UPDATE_FS, KERNEL_CURRENT, KERNEL_OLD).unwrap();
        vfs.rename_fs(UPDATE_FS, KERNEL_NEW, KERNEL_CURRENT).unwrap();

        let uart = UART.get();
        uart.write_text("COMMIT DONE, REBOOTING\n");

        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }

        WATCHDOG.get().reboot();
    }

    fn verify_magic_bytes(image: &[u8]) -> Result<(), UpdateError> {
        if image.len() < 4 {
            return Err(UpdateError::InvalidImage);
        }

        let off = image.len() - 4;

        let magic = u32::from_le_bytes([
            image[off],
            image[off + 1],
            image[off + 2],
            image[off + 3],
        ]);

        if magic != UPDATE_MAGIC {
            return Err(UpdateError::BadMagic);
        }

        Ok(())
    }
}