extern crate alloc;

use alloc::vec::Vec;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::partition::{Partition, PartitionError, PartitionKind, PartitionScheme};

#[inline(always)]
fn le_u32(buf: &[u8; SECTOR_SIZE], off: usize) -> u32 {
    (buf[off] as u32)
        | ((buf[off + 1] as u32) << 8)
        | ((buf[off + 2] as u32) << 16)
        | ((buf[off + 3] as u32) << 24)
}

/// Reads MBR at LBA0 and returns a Vec of primary partitions (non-empty entries).
///
/// Notes:
/// - Skips empty entries (type==0 or sector_count==0)
/// - Does not parse extended partitions yet
pub fn read_partitions(dev: &dyn BlockDevice) -> Result<Vec<Partition>, PartitionError> {
    let mut lba0 = [0u8; SECTOR_SIZE];
    dev.read_sector(0, &mut lba0)?;

    let sig = (lba0[510] as u16) | ((lba0[511] as u16) << 8);
    if sig != 0xAA55 {
        return Err(PartitionError::InvalidSignature(sig));
    }

    const PT_OFF: usize = 446;
    const ENTRY_SIZE: usize = 16;

    let mut out = Vec::new();

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

        // Basic sanity
        if start_lba == 0 {
            return Err(PartitionError::Invalid(
                "MBR: non-empty entry has start_lba=0",
            ));
        }

        // Optional bounds checks if device knows its size
        if let Some(total) = dev.sector_count() {
            if start_lba >= total {
                return Err(PartitionError::Block(
                    crate::storage::block::BlockError::OutOfBounds(start_lba),
                ));
            }
            if start_lba.saturating_add(sectors) > total {
                return Err(PartitionError::Block(
                    crate::storage::block::BlockError::OutOfBounds(
                        start_lba.saturating_add(sectors),
                    ),
                ));
            }
        }

        out.push(Partition {
            index: (i + 1) as u32,
            scheme: PartitionScheme::Mbr,
            kind: PartitionKind::MbrType(part_type),
            start_lba,
            sector_count: sectors,
            bootable: boot == 0x80,
        });
    }

    Ok(out)
}

/// Returns true if this MBR looks like a GPT protective MBR (contains a 0xEE entry).
pub fn is_protective_mbr(parts: &[Partition]) -> bool {
    parts
        .iter()
        .any(|p| matches!(p.kind, PartitionKind::MbrType(0xEE)))
}
