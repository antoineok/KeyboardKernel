extern crate alloc;

use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};

//--------------------------------------------------------------------------------------------------
// Partition Schemes
//--------------------------------------------------------------------------------------------------
pub mod mbr;
// pub mod gpt; // later

//--------------------------------------------------------------------------------------------------
// Common partition types
//--------------------------------------------------------------------------------------------------
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionScheme {
    Mbr,
    Gpt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionKind {
    MbrType(u8),
    // GptTypeGuid(Guid), // later
}

#[derive(Clone, Copy, Debug)]
pub struct Partition {
    /// 1-based index (like Linux: sde1 => 1)
    pub index: u32,

    pub scheme: PartitionScheme,
    pub kind: PartitionKind,

    pub start_lba: u64,
    pub sector_count: u64,

    /// MBR uses this. For GPT it can always be false.
    pub bootable: bool,
}

impl Partition {
    /// Create a partition-relative block device view.
    #[inline(always)]
    pub fn as_block_device<'a>(&self, parent: &'a dyn BlockDevice) -> PartitionBlockDevice<'a> {
        PartitionBlockDevice::new(parent, self.start_lba, self.sector_count)
    }
}

#[derive(Debug)]
pub enum PartitionError {
    Block(BlockError),
    Invalid(&'static str),
    InvalidSignature(u16),
}

impl From<BlockError> for PartitionError {
    fn from(e: BlockError) -> Self {
        PartitionError::Block(e)
    }
}

//--------------------------------------------------------------------------------------------------
// Partition slice block device
//--------------------------------------------------------------------------------------------------
pub struct PartitionBlockDevice<'a> {
    parent: &'a dyn BlockDevice,
    start_lba: u64,
    sector_count: u64,
}

impl<'a> PartitionBlockDevice<'a> {
    pub fn new(parent: &'a dyn BlockDevice, start_lba: u64, sector_count: u64) -> Self {
        Self {
            parent,
            start_lba,
            sector_count,
        }
    }

    #[inline(always)]
    fn map_lba(&self, rel_sector: u64) -> Result<u64, BlockError> {
        if rel_sector >= self.sector_count {
            // You chose absolute LBA reporting ✅
            return Err(BlockError::OutOfBounds(self.start_lba + rel_sector));
        }
        Ok(self.start_lba + rel_sector)
    }

    pub fn start_lba(&self) -> u64 {
        self.start_lba
    }

    pub fn sector_count_abs(&self) -> u64 {
        self.sector_count
    }
}

impl<'a> BlockDevice for PartitionBlockDevice<'a> {
    fn sector_size(&self) -> usize {
        self.parent.sector_size()
    }

    fn read_sector(&self, sector: u64, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let abs = self.map_lba(sector)?;
        self.parent.read_sector(abs, buffer)
    }

    fn write_sector(&self, sector: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), BlockError> {
        let abs = self.map_lba(sector)?;
        self.parent.write_sector(abs, data)
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.parent.flush()
    }

    fn sector_count(&self) -> Option<u64> {
        Some(self.sector_count)
    }
}

//--------------------------------------------------------------------------------------------------
// Runtime diagnostics helpers (boot-time tests)
//--------------------------------------------------------------------------------------------------

/// Print a FAT32-style VBR label/OEM by reading sector 0 of a *partition slice*.
/// This is purely for bring-up / testing. (Real FAT32 will parse more fields later.)
pub fn diag_print_fat32_vbr(dev: &dyn BlockDevice, part: &Partition) {
    use crate::{info, warn};

    let slice = part.as_block_device(dev);

    let mut vbr = [0u8; SECTOR_SIZE];
    if let Err(e) = slice.read_sector(0, &mut vbr) {
        warn!(
            "Partition {}: failed to read VBR via slice (abs start {}): {:?}",
            part.index, part.start_lba, e
        );
        return;
    }

    let sig = (vbr[510] as u16) | ((vbr[511] as u16) << 8);
    if sig != 0xAA55 {
        warn!(
            "Partition {}: VBR signature invalid (got 0x{:04x})",
            part.index, sig
        );
        return;
    }

    let oem = &vbr[0x03..0x0B];
    let label = &vbr[0x47..0x47 + 11];

    fn trim_ascii(bytes: &[u8]) -> &str {
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1] == b' ' {
            end -= 1;
        }
        core::str::from_utf8(&bytes[..end]).unwrap_or("<non-utf8>")
    }

    info!(
        "Partition {} FAT32 VBR (slice sector 0): OEM='{}' Label='{}'",
        part.index,
        trim_ascii(oem),
        trim_ascii(label)
    );
}

/// Full partition table + FAT32 VBR label test (boot-time).
pub fn storage_partition_self_test(dev: &dyn BlockDevice) {
    use crate::{info, warn};

    info!("=== Partition self-test (MBR) ===");

    let parts = match mbr::read_partitions(dev) {
        Ok(v) => v,
        Err(e) => {
            warn!("MBR read_partitions failed: {:?}", e);
            return;
        }
    };

    if parts.is_empty() {
        warn!("No MBR partitions found");
        return;
    }

    for p in &parts {
        let (scheme, kind) = (p.scheme, p.kind);
        info!(
            "Partition {}: scheme={:?} kind={:?} boot={} start={} sectors={}",
            p.index,
            scheme,
            kind,
            if p.bootable { "yes" } else { "no" },
            p.start_lba,
            p.sector_count
        );

        diag_test_slice_bounds(dev, p);

        // If it's FAT32 (0x0B / 0x0C), print label from VBR via slice
        if let PartitionKind::MbrType(t) = p.kind {
            if t == 0x0B || t == 0x0C {
                diag_print_fat32_vbr(dev, p);
            }
        }
    }

    info!("=== Partition self-test done ===");
}

fn diag_test_slice_bounds(dev: &dyn BlockDevice, part: &Partition) {
    use crate::storage::block::BlockError;
    use crate::{info, warn};

    let slice = part.as_block_device(dev);

    // One past the end (invalid)
    let rel_oob = part.sector_count;
    let expected_abs = part.start_lba + rel_oob;

    let mut buf = [0u8; SECTOR_SIZE];
    match slice.read_sector(rel_oob, &mut buf) {
        Ok(_) => {
            warn!(
                "Partition {}: OOB test FAILED: read_sector(rel={}) unexpectedly succeeded",
                part.index, rel_oob
            );
        }
        Err(BlockError::OutOfBounds(abs)) => {
            if abs == expected_abs {
                info!(
                    "Partition {}: OOB test OK (rel={} -> OutOfBounds(abs=0x{:016x}))",
                    part.index, rel_oob, abs
                );
            } else {
                warn!(
                    "Partition {}: OOB test mismatch: got abs=0x{:016x}, expected 0x{:016x}",
                    part.index, abs, expected_abs
                );
            }
        }
        Err(e) => {
            warn!(
                "Partition {}: OOB test FAILED: unexpected error {:?}",
                part.index, e
            );
        }
    }
}
