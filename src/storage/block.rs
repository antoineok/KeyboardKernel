use core::fmt::Debug;

use crate::drivers::emmc::EMMC;

pub const SECTOR_SIZE: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    Unsupported,
    Timeout,
    OutOfBounds(u64),
    Io,
    Corrupt(&'static str),
}

impl Debug for BlockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BlockError::Unsupported => write!(f, "BlockError::Unsupported"),
            BlockError::Timeout => write!(f, "BlockError::Timeout"),
            BlockError::OutOfBounds(sector) => {
                write!(f, "BlockError::OutOfBounds({:016x})", sector)
            }
            BlockError::Io => write!(f, "BlockError::Io"),
            BlockError::Corrupt(msg) => write!(f, "BlockError::Corrupt({})", msg),
        }
    }
}

pub trait BlockDevice {
    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn read_sector(&self, sector: u64, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError>;
    fn write_sector(&self, sector: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), BlockError>;

    fn flush(&self) -> Result<(), BlockError> {
        // Implementation for flushing eMMC writes
        Err(BlockError::Unsupported)
    }

    fn sector_count(&self) -> Option<u64>;
}

pub struct EmmcBlockDevice;

impl EmmcBlockDevice {
    pub const fn new() -> Self {
        Self
    }

    #[inline(always)]
    fn lba_u32(&self, sector: u64) -> Result<u32, BlockError> {
        if sector > u32::MAX as u64 {
            return Err(BlockError::OutOfBounds(sector));
        }
        Ok(sector as u32)
    }
}

impl BlockDevice for EmmcBlockDevice {
    fn read_sector(&self, sector: u64, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let sector: u32 = self.lba_u32(sector)?;
        EMMC.get()
            .read_block(sector, buffer)
            .map_err(|error| match error {
                crate::drivers::emmc::EmmcError::Timeout(_) => BlockError::Timeout,
                crate::drivers::emmc::EmmcError::NotInitialized => {
                    BlockError::Corrupt("emmc not initialized")
                }
                _ => BlockError::Io,
            })?;
        Ok(())
    }

    fn write_sector(&self, sector: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let sector: u32 = self.lba_u32(sector)?;
        EMMC.get()
            .write_block(sector, data)
            .map_err(|error| match error {
                crate::drivers::emmc::EmmcError::Timeout(_) => BlockError::Timeout,
                crate::drivers::emmc::EmmcError::NotInitialized => {
                    BlockError::Corrupt("emmc not initialized")
                }
                _ => BlockError::Io,
            })?;
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        // Implementation for flushing eMMC writes
        Ok(())
    }

    fn sector_count(&self) -> Option<u64> {
        // Return the total number of sectors in the eMMC device
        None
    }
}

pub const EMMC_BLOCK: EmmcBlockDevice = EmmcBlockDevice::new();
