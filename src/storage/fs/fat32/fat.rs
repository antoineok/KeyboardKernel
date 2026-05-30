use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

/// FAT32 uses 28 bits of the 32-bit entry.
const FAT32_MASK: u32 = 0x0FFF_FFFF;

/// FAT32 special values (masked to 28-bit domain).
const FAT32_FREE: u32 = 0x0000_0000;
const FAT32_BAD: u32 = 0x0FFF_FFF7;
const FAT32_EOC_MIN: u32 = 0x0FFF_FFF8; // End-of-chain threshold

const FAT32_EOC: u32 = 0x0FFF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FatLink {
    /// Next cluster in chain.
    Next(u32),
    /// End-of-chain marker.
    Eoc,
    /// Free/unused cluster.
    Free,
    /// Bad cluster.
    Bad,
    /// Reserved/special value (rare). We treat as EOC-ish to be safe.
    Reserved(u32),
}

impl Fat32Volume {
    /// Read one FAT32 entry for `cluster` from FAT#1.
    ///
    /// Returns a classified link (Next/Eoc/etc).
    pub fn fat32_get_link(
        &self,
        dev: &dyn BlockDevice,
        cluster: u32,
    ) -> Result<FatLink, Fat32Error> {
        if cluster < 2 {
            return Err(Fat32Error::Invalid("FAT32: cluster < 2"));
        }

        // FAT32 entry index is cluster number, 4 bytes each.
        let byte_off = (cluster as u64) * 4;
        let sector_off = byte_off / (SECTOR_SIZE as u64);
        let in_sector = (byte_off % (SECTOR_SIZE as u64)) as usize;

        // FAT starts at fat1_lba (partition-relative)
        let fat_sector_lba = self.fat1_lba + sector_off;

        let mut sec = [0u8; SECTOR_SIZE];
        dev.read_sector(fat_sector_lba, &mut sec)?;

        if in_sector + 4 > SECTOR_SIZE {
            // Should never happen for 4-byte entries in 512-byte sectors, but guard anyway.
            return Err(Fat32Error::Invalid(
                "FAT32: FAT entry crosses sector boundary",
            ));
        }

        let raw = u32::from_le_bytes([
            sec[in_sector],
            sec[in_sector + 1],
            sec[in_sector + 2],
            sec[in_sector + 3],
        ]);
        let val = raw & FAT32_MASK;

        let link = if val == 0x0000_0000 {
            FatLink::Free
        } else if val == 0x0000_0001 {
            FatLink::Reserved(val)
        } else if val >= 0x0000_0002 && val <= 0x0FFF_FFEF {
            FatLink::Next(val)
        } else if val >= 0x0FFF_FFF0 && val <= 0x0FFF_FFF6 {
            FatLink::Reserved(val)
        } else if val == 0x0FFF_FFF7 {
            FatLink::Bad
        } else if val >= 0x0FFF_FFF8 {
            FatLink::Eoc
        } else {
            FatLink::Reserved(val)
        };

        Ok(link)
    }

    /// A safe iterator-like helper: follow a cluster chain starting at `start`.
    ///
    /// Calls `mut f(cluster)` for each cluster in the chain, in order.
    ///
    /// Stops at EOC. Errors on loops or bad/free clusters.
    pub fn follow_cluster_chain<F: FnMut(u32) -> Result<(), Fat32Error>>(
        &self,
        dev: &dyn BlockDevice,
        start: u32,
        mut f: F,
    ) -> Result<(), Fat32Error> {
        if start < 2 {
            return Err(Fat32Error::Invalid("FAT32: chain start < 2"));
        }

        // Upper bound to prevent infinite loops if FAT is corrupted:
        // max clusters ~= tot_sec / sec_per_clus
        let max_steps = (self.bpb.tot_sec as u64 / self.bpb.sec_per_clus as u64).max(1);

        let mut cur = start;
        for _ in 0..max_steps {
            f(cur)?;

            match self.fat32_get_link(dev, cur)? {
                FatLink::Next(n) => {
                    // FAT corruption: self-loop
                    if n == cur {
                        return Err(Fat32Error::Invalid("FAT32: FAT self-loop detected"));
                    }
                    cur = n;
                }
                FatLink::Eoc => return Ok(()),
                FatLink::Bad => return Err(Fat32Error::Invalid("FAT32: encountered BAD cluster")),
                FatLink::Free => {
                    return Err(Fat32Error::Invalid(
                        "FAT32: encountered FREE cluster in chain",
                    ))
                }
                FatLink::Reserved(v) => {
                    // Conservative: treat reserved as end-of-chain rather than walking into garbage
                    let _ = v;
                    return Ok(());
                }
            }
        }

        Err(Fat32Error::Invalid(
            "FAT32: cluster chain too long (possible loop/corruption)",
        ))
    }

    fn fat32_set_raw(
        &self,
        dev: &dyn BlockDevice,
        cluster: u32,
        value: u32,
    ) -> Result<(), Fat32Error> {
        if cluster < 2 {
            return Err(Fat32Error::Invalid("FAT32: cluster < 2"));
        }

        let byte_off = cluster as u64 * 4;
        let sector_off = byte_off / SECTOR_SIZE as u64;
        let in_sector = (byte_off % SECTOR_SIZE as u64) as usize;

        let val = value & FAT32_MASK;

        for fat_index in 0..self.bpb.num_fats {
            let lba = self.fat1_lba + fat_index as u64 * self.bpb.fat_sz as u64 + sector_off;

            let mut sec = [0u8; SECTOR_SIZE];
            dev.read_sector(lba, &mut sec)?;
            sec[in_sector..in_sector + 4].copy_from_slice(&val.to_le_bytes());
            dev.write_sector(lba, &sec)?;
        }

        Ok(())
    }

    pub fn fat32_set_eoc(&self, dev: &dyn BlockDevice, cluster: u32) -> Result<(), Fat32Error> {
        self.fat32_set_raw(dev, cluster, FAT32_EOC)
    }

    pub fn fat32_set_next(
        &self,
        dev: &dyn BlockDevice,
        cluster: u32,
        next: u32,
    ) -> Result<(), Fat32Error> {
        self.fat32_set_raw(dev, cluster, next)
    }

    pub fn fat32_set_free(&self, dev: &dyn BlockDevice, cluster: u32) -> Result<(), Fat32Error> {
        self.fat32_set_raw(dev, cluster, FAT32_FREE)
    }

    pub fn fat32_find_free_cluster(&mut self, dev: &dyn BlockDevice) -> Result<u32, Fat32Error> {
        let max_clusters = self.bpb.tot_sec / self.bpb.sec_per_clus as u32;

        let start = self.next_free_hint.max(2);

        for cluster in start..max_clusters {
            if self.fat32_get_link(dev, cluster)? == FatLink::Free {
                self.next_free_hint = cluster + 1;
                return Ok(cluster);
            }
        }

        for cluster in 2..start {
            if self.fat32_get_link(dev, cluster)? == FatLink::Free {
                self.next_free_hint = cluster + 1;
                return Ok(cluster);
            }
        }

        Err(Fat32Error::Invalid("FAT32: no free cluster"))
    }

    pub fn fat32_alloc_cluster(&mut self, dev: &dyn BlockDevice) -> Result<u32, Fat32Error> {
        let cluster = self.fat32_find_free_cluster(dev)?;
        self.fat32_set_eoc(dev, cluster)?;
        self.clear_cluster(dev, cluster)?;
        Ok(cluster)
    }

    pub fn clear_cluster(&self, dev: &dyn BlockDevice, cluster: u32) -> Result<(), Fat32Error> {
        let base = self.cluster_to_lba(cluster)?;
        let zero = [0u8; SECTOR_SIZE];

        for s in 0..self.bpb.sec_per_clus as u64 {
            dev.write_sector(base + s, &zero)?;
        }

        Ok(())
    }
}
