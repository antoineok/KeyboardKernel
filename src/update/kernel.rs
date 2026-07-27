use crate::{
    drivers::watchdog::WATCHDOG,
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

        VFS.with_mut(|vfs| -> Result<(), UpdateError> {
            if vfs.exists_fs(UPDATE_FS, KERNEL_NEW) {
                vfs.delete_fs(UPDATE_FS, KERNEL_NEW)?;
            }
            vfs.write_fs(UPDATE_FS, KERNEL_NEW, image)?;
            Ok(())
        })
    }

    pub fn verify_staged(
        expected_size: usize,
        expected_sha256: &[u8; 32],
    ) -> Result<(), UpdateError> {
        Self::verify_file(KERNEL_NEW, expected_size, expected_sha256)
            .map_err(|error| match error {
                UpdateError::Vfs(_) => UpdateError::MissingStagedImage,
                other => other,
            })
    }

    pub fn discard_staged() -> Result<(), UpdateError> {
        VFS.with_mut(|vfs| -> Result<(), UpdateError> {
            if vfs.exists_fs(UPDATE_FS, KERNEL_NEW) {
                vfs.delete_fs(UPDATE_FS, KERNEL_NEW)?;
            }
            Ok(())
        })
    }

    pub fn has_staged() -> bool {
        VFS.with_ref(|vfs| vfs.exists_fs(UPDATE_FS, KERNEL_NEW))
    }

    pub fn commit_and_reboot() -> ! {
        match Self::commit_staged() {
            Ok((size, hash)) => {
                crate::info!(
                    "Firmware activation durable and verified: bytes={}, sha256={:02x}{:02x}{:02x}{:02x}...; rebooting",
                    size,
                    hash[0],
                    hash[1],
                    hash[2],
                    hash[3]
                );
                WATCHDOG.get().reboot();
            }
            Err(error) => {
                crate::error!("Firmware activation failed; refusing reboot: {:?}", error);
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    }

    fn commit_staged() -> Result<(usize, [u8; 32]), UpdateError> {
        let (expected_size, expected_sha256) = Self::fingerprint_file(KERNEL_NEW)?;

        VFS.with_mut(|vfs| -> Result<(), UpdateError> {
            if vfs.exists_fs(UPDATE_FS, KERNEL_OLD) {
                vfs.delete_fs(UPDATE_FS, KERNEL_OLD)?;
            }

            vfs.rename_fs(UPDATE_FS, KERNEL_CURRENT, KERNEL_OLD)?;

            if let Err(error) = vfs.rename_fs(UPDATE_FS, KERNEL_NEW, KERNEL_CURRENT) {
                // Do not leave the boot filesystem without kernel8.img if the
                // second half of the swap fails.
                let _ = vfs.rename_fs(UPDATE_FS, KERNEL_OLD, KERNEL_CURRENT);
                let _ = vfs.flush_fs(UPDATE_FS);
                return Err(error.into());
            }

            // Explicit durability boundary after the complete directory swap.
            vfs.flush_fs(UPDATE_FS)?;
            Ok(())
        })?;

        if let Err(error) = Self::verify_file(KERNEL_CURRENT, expected_size, &expected_sha256) {
            Self::rollback_to_old();
            return Err(error);
        }

        // Verify reads happened after a durable flush. Flush once more before
        // watchdog reset so there is no outstanding filesystem write left.
        VFS.with_ref(|vfs| vfs.flush_fs(UPDATE_FS))?;
        Ok((expected_size, expected_sha256))
    }

    fn rollback_to_old() {
        VFS.with_mut(|vfs| {
            if vfs.exists_fs(UPDATE_FS, KERNEL_CURRENT) {
                let _ = vfs.delete_fs(UPDATE_FS, KERNEL_CURRENT);
            }
            if vfs.exists_fs(UPDATE_FS, KERNEL_OLD) {
                let _ = vfs.rename_fs(UPDATE_FS, KERNEL_OLD, KERNEL_CURRENT);
            }
            let _ = vfs.flush_fs(UPDATE_FS);
        });
    }

    fn fingerprint_file(path: &str) -> Result<(usize, [u8; 32]), UpdateError> {
        VFS.with_ref(|vfs| -> Result<(usize, [u8; 32]), UpdateError> {
            let file = vfs.open_fs(UPDATE_FS, path)?;
            Self::verify_magic_bytes(file.as_bytes())?;
            Ok((file.len(), sha256(file.as_bytes())))
        })
    }

    fn verify_file(
        path: &str,
        expected_size: usize,
        expected_sha256: &[u8; 32],
    ) -> Result<(), UpdateError> {
        VFS.with_ref(|vfs| -> Result<(), UpdateError> {
            let file = vfs.open_fs(UPDATE_FS, path)?;

            if file.len() != expected_size {
                return Err(UpdateError::VerifyFailed);
            }
            Self::verify_magic_bytes(file.as_bytes())?;
            if &sha256(file.as_bytes()) != expected_sha256 {
                return Err(UpdateError::VerifyFailed);
            }
            Ok(())
        })
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
