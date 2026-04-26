use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};

/// MBR partitions are described in LBA terms.
#[derive(Clone, Copy, Debug)]
pub struct MbrPartition {
    /// 1..=4 (MBR has exactly 4 primary entries)
    pub index: u8,

    /// True if the bootable flag is 0x80
    pub bootable: bool,

    /// MBR partition type byte (e.g., 0x0C = FAT32 LBA, 0x83 = Linux)
    pub part_type: u8,

    /// First LBA (relative to disk start)
    pub start_lba: u64,

    /// Number of sectors in partition
    pub sector_count: u64,
}

#[derive(Debug)]
pub enum MbrError {
    Block(BlockError),
    InvalidSignature(u16),
    Corrupt(&'static str),
}

impl From<BlockError> for MbrError {
    fn from(e: BlockError) -> Self {
        MbrError::Block(e)
    }
}

/// Read little-endian u32 from `buf[offset..offset+4]`.
#[inline(always)]
fn le_u32(buf: &[u8; SECTOR_SIZE], offset: usize) -> u32 {
    let b0 = buf[offset] as u32;
    let b1 = buf[offset + 1] as u32;
    let b2 = buf[offset + 2] as u32;
    let b3 = buf[offset + 3] as u32;
    b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
}

/// Parse the MBR at LBA0 and return all non-empty primary partitions.
///
/// - Validates 0x55AA signature.
/// - Skips empty entries (type==0 or sector_count==0).
/// - Does NOT attempt to interpret extended partitions (0x05/0x0F) yet.
///   (We can add that later if you need it.)
pub fn parse_mbr(dev: &dyn BlockDevice) -> Result<[Option<MbrPartition>; 4], MbrError> {
    let mut lba0 = [0u8; SECTOR_SIZE];
    dev.read_sector(0, &mut lba0)?;

    let sig = (lba0[510] as u16) | ((lba0[511] as u16) << 8);
    if sig != 0xAA55 {
        return Err(MbrError::InvalidSignature(sig));
    }

    // MBR partition table starts at offset 446, 4 entries of 16 bytes each.
    const PT_OFF: usize = 446;
    const ENTRY_SIZE: usize = 16;

    let mut out: [Option<MbrPartition>; 4] = [None, None, None, None];

    for i in 0..4 {
        let off = PT_OFF + i * ENTRY_SIZE;

        let boot = lba0[off + 0];
        let part_type = lba0[off + 4];

        let start_lba = le_u32(&lba0, off + 8) as u64;
        let sectors = le_u32(&lba0, off + 12) as u64;

        // Empty entry?
        if part_type == 0 || sectors == 0 {
            continue;
        }

        // Basic sanity checks
        if start_lba == 0 {
            return Err(MbrError::Corrupt("MBR partition has start_lba=0 but is non-empty"));
        }

        // Optional bounds check if we know device size
        if let Some(total) = dev.sector_count() {
            if start_lba >= total {
                return Err(MbrError::Block(BlockError::OutOfBounds(start_lba)));
            }
            if start_lba.saturating_add(sectors) > total {
                return Err(MbrError::Block(BlockError::OutOfBounds(
                    start_lba.saturating_add(sectors),
                )));
            }
        }

        out[i] = Some(MbrPartition {
            index: (i + 1) as u8,
            bootable: boot == 0x80,
            part_type,
            start_lba,
            sector_count: sectors,
        });
    }

    Ok(out)
}

/// Convenience: return a Vec of partitions (easier to iterate).
pub fn list_mbr_partitions(dev: &dyn BlockDevice) -> Result<alloc::vec::Vec<MbrPartition>, MbrError> {
    // Requires alloc; you said heap is ready by the time you call it.
    let arr = parse_mbr(dev)?;
    let mut v = alloc::vec::Vec::new();
    for p in arr.iter().flatten() {
        v.push(*p);
    }
    Ok(v)
}

/// True if the MBR appears to be a GPT protective MBR (has a 0xEE partition type).
pub fn is_protective_mbr(parts: &[Option<MbrPartition>; 4]) -> bool {
    parts.iter().flatten().any(|p| p.part_type == 0xEE)
}
