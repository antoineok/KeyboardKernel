use core::time::Duration;

use crate::{
    drivers::watchdog::WATCHDOG,
    storage::fs::vfs::{FsId, VFS},
    time::time_manager,
};

use super::{sha256::sha256, UpdateError, UPDATE_MAGIC};

const KERNEL_CURRENT: &str = "kernel8.img";
const KERNEL_NEW: &str = "kernel8.new";
const KERNEL_OLD: &str = "kernel8.old";
const UPDATE_FS: FsId = FsId::Boot;
/// Give the eMMC a quiet interval after the final verified durability barrier.
/// The VFS lock is intentionally held during this delay and through reset so a
/// graphics-core background prefetch cannot start another storage transaction.
const RESET_STORAGE_QUIESCE: Duration = Duration::from_millis(250);

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
                    "Firmware activation durable and verified: bytes={}, sha256={:02x}{:02x}{:02x}{:02x}...; quiescing storage before reboot",
                    size,
                    hash[0],
                    hash[1],
                    hash[2],
                    hash[3]
                );

                // Hold the VFS lock from the final durability barrier through
                // watchdog reset. CPU1 may still be progressively filling a
                // resident video from eMMC; allowing a new transaction after
                // the updater's last flush defeats the meaning of a quiescent
                // reset boundary.
                VFS.with_ref(|vfs| {
                    vfs.flush_fs(UPDATE_FS)
                        .expect("final firmware activation flush failed");
                    time_manager().spin_for(RESET_STORAGE_QUIESCE);
                    WATCHDOG.get().reboot();
                });
                unreachable!("watchdog reboot returned");
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
        // Keep the verified staged bytes in RAM while activation runs. Unlike
        // the old rename swap, overwrite kernel8.img in place so the firmware
        // bootloader continues to see the same directory entry and starting
        // cluster it has always used.
        let staged = VFS.with_ref(|vfs| vfs.open_fs(UPDATE_FS, KERNEL_NEW))?;
        Self::verify_magic_bytes(staged.as_bytes())?;
        let expected_size = staged.len();
        let expected_sha256 = sha256(staged.as_bytes());

        let current = VFS.with_ref(|vfs| vfs.open_fs(UPDATE_FS, KERNEL_CURRENT))?;

        VFS.with_mut(|vfs| -> Result<(), UpdateError> {
            if vfs.exists_fs(UPDATE_FS, KERNEL_OLD) {
                vfs.delete_fs(UPDATE_FS, KERNEL_OLD)?;
            }

            // Keep a byte-for-byte rollback copy before touching the active
            // boot filename. The backup uses its own FAT chain.
            vfs.write_fs(UPDATE_FS, KERNEL_OLD, current.as_bytes())?;
            vfs.flush_fs(UPDATE_FS)?;

            // Preserve kernel8.img's directory slot and first cluster. The FAT
            // writer grows/shrinks its existing chain only when necessary.
            vfs.write_fs(UPDATE_FS, KERNEL_CURRENT, staged.as_bytes())?;
            vfs.flush_fs(UPDATE_FS)?;
            Ok(())
        })?;

        if let Err(error) = Self::verify_file(KERNEL_CURRENT, expected_size, &expected_sha256) {
            Self::rollback_from_old();
            return Err(error);
        }

        // The staged filename is no longer part of activation. Remove it only
        // after the active filename has passed a post-flush SHA-256 reread.
        VFS.with_mut(|vfs| -> Result<(), UpdateError> {
            if vfs.exists_fs(UPDATE_FS, KERNEL_NEW) {
                vfs.delete_fs(UPDATE_FS, KERNEL_NEW)?;
            }
            vfs.flush_fs(UPDATE_FS)?;
            Ok(())
        })?;

        Ok((expected_size, expected_sha256))
    }

    fn rollback_from_old() {
        let backup = VFS.with_ref(|vfs| vfs.open_fs(UPDATE_FS, KERNEL_OLD));
        let Ok(backup) = backup else {
            return;
        };

        VFS.with_mut(|vfs| {
            let _ = vfs.write_fs(UPDATE_FS, KERNEL_CURRENT, backup.as_bytes());
            let _ = vfs.flush_fs(UPDATE_FS);
        });
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
