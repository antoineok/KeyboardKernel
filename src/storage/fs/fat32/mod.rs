pub mod create;
pub mod delete;
pub mod rename;
pub mod bpb;
pub mod fat;
pub mod dir;
pub mod file;
pub mod path;

use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};
use crate::storage::fs::fat32::bpb::Fat32Bpb;

#[derive(Debug)]
pub enum Fat32Error {
    Block(BlockError),
    Invalid(&'static str),
}

impl From<BlockError> for Fat32Error {
    fn from(e: BlockError) -> Self {
        Fat32Error::Block(e)
    }
}

/// Mounted FAT32 volume, computed from BPB.
#[derive(Clone, Copy, Debug)]
pub struct Fat32Volume {
    pub bpb: Fat32Bpb,

    /// Partition-relative LBA where FAT #1 starts.
    pub fat1_lba: u64,

    /// Partition-relative LBA where the data region starts (cluster #2).
    pub data_lba: u64,

    next_free_hint: u32,
}

impl Fat32Volume {
    /// Mount FAT32 from a device where sector 0 is the VBR.
    pub fn mount(dev: &dyn BlockDevice) -> Result<Self, Fat32Error> {
        let mut vbr = [0u8; SECTOR_SIZE];
        dev.read_sector(0, &mut vbr)?;

        let bpb = Fat32Bpb::parse(&vbr)?;

        let fat1_lba = bpb.rsvd_sec_cnt as u64;
        let data_lba =
            fat1_lba + (bpb.num_fats as u64) * (bpb.fat_sz as u64);

        Ok(Self { bpb, fat1_lba, data_lba, next_free_hint: 2 })
    }

    /// Convert a cluster number to a partition-relative LBA (first sector of that cluster).
    #[inline(always)]
    pub fn cluster_to_lba(&self, cluster: u32) -> Result<u64, Fat32Error> {
        if cluster < 2 {
            return Err(Fat32Error::Invalid("cluster < 2 is invalid in FAT32"));
        }
        let cluster_index = (cluster - 2) as u64;
        Ok(self.data_lba + cluster_index * (self.bpb.sec_per_clus as u64))
    }
}