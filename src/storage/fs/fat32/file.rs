extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};
use crate::storage::fs::fat32::fat::FatLink;
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};
use crate::info;

#[derive(Clone, Debug)]
pub struct FatFile {
    pub name: String,
    pub first_cluster: u32,
    pub size: u32,

    /// Partition-relative LBA of the sector containing the 32-byte 8.3 entry.
    pub dirent_lba: u64,

    /// Byte offset within that sector (0..512-32, multiple of 32).
    pub dirent_off: usize,
}

impl Fat32Volume {
    //----------------------------------------------------------------------------------------------
    // Small helpers
    //----------------------------------------------------------------------------------------------

    /// Case-insensitive ASCII compare.
    fn eq_name_ci(a: &str, b: &str) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
    }

    #[inline(always)]
    fn is_eoc(v: u32) -> bool {
        // FAT32 EOC is typically >= 0x0FFFFFF8 (only low 28 bits meaningful)
        (v & 0x0FFF_FFFF) >= 0x0FFF_FFF8
    }

    #[inline(always)]
    fn fat_next_from_link(link: FatLink) -> Result<Option<u32>, Fat32Error> {
        match link {
            FatLink::Next(n) => Ok(Some(n)),
            FatLink::Eoc => Ok(None),
            FatLink::Bad => Err(Fat32Error::Invalid("BAD cluster in file chain")),
            FatLink::Free => Err(Fat32Error::Invalid("FREE cluster in file chain")),
            FatLink::Reserved(_) => Ok(None), // conservative
        }
    }

    fn map_block_err(e: BlockError) -> Fat32Error {
        // Keep your existing Fat32Error style. If you have Fat32Error::Block, use it.
        // Otherwise map to Invalid for now.
        match e {
            BlockError::Timeout => Fat32Error::Invalid("block timeout"),
            BlockError::OutOfBounds(_) => Fat32Error::Invalid("block out of bounds"),
            BlockError::Corrupt(msg) => Fat32Error::Invalid(msg),
            _ => Fat32Error::Invalid("block io error"),
        }
    }

    //----------------------------------------------------------------------------------------------
    // Open helpers
    //----------------------------------------------------------------------------------------------

    /// Read up to `out.len()` bytes from a file, starting at offset 0.
    /// Returns number of bytes written to `out`.
    pub fn read_file_prefix(
        &self,
        dev: &dyn BlockDevice,
        file: &FatFile,
        out: &mut [u8],
    ) -> Result<usize, Fat32Error> {
        if file.first_cluster < 2 {
            return Err(Fat32Error::Invalid("file has invalid first_cluster"));
        }

        let mut remaining = core::cmp::min(out.len(), file.size as usize);
        let mut out_off = 0usize;

        let spc = self.bpb.sec_per_clus as u64;

        // Walk cluster chain
        let mut cur = file.first_cluster;

        // Upper bound to avoid infinite loops
        let max_steps = (self.bpb.tot_sec as u64 / self.bpb.sec_per_clus as u64).max(1);

        for _ in 0..max_steps {
            if remaining == 0 {
                break;
            }

            let base_lba = self.cluster_to_lba(cur)?;
            for s in 0..spc {
                if remaining == 0 {
                    break;
                }

                let mut sec = [0u8; SECTOR_SIZE];
                dev.read_sector(base_lba + s, &mut sec)
                    .map_err(Self::map_block_err)?;

                let take = core::cmp::min(remaining, SECTOR_SIZE);
                out[out_off..out_off + take].copy_from_slice(&sec[..take]);

                out_off += take;
                remaining -= take;
            }

            if remaining == 0 {
                break;
            }

            match self.fat32_get_link(dev, cur)? {
                FatLink::Next(n) => {
                    if n == cur {
                        return Err(Fat32Error::Invalid("FAT self-loop in file chain"));
                    }
                    cur = n;
                }
                FatLink::Eoc => break,
                FatLink::Bad => return Err(Fat32Error::Invalid("BAD cluster in file chain")),
                FatLink::Free => return Err(Fat32Error::Invalid("FREE cluster in file chain")),
                FatLink::Reserved(_) => break, // conservative
            }
        }

        Ok(out_off)
    }

        pub fn exists(
            &self,
            dev: &dyn BlockDevice,
            path: &str,
        ) -> bool {
            self.resolve_path(dev, path).is_ok()
        }

    //----------------------------------------------------------------------------------------------
    // Write (phase 1): overwrite existing file, no allocation
    //----------------------------------------------------------------------------------------------

    /// Overwrite an existing file from offset 0.
    ///
    /// **Phase 1 constraint:** `data.len()` must fit in the already allocated cluster chain.
    /// We do NOT allocate/free clusters yet.
    pub fn write_file_all(
        &self,
        dev: &dyn BlockDevice,
        file: &FatFile,
        data: &[u8],
    ) -> Result<(), Fat32Error> {
        if file.first_cluster < 2 {
            return Err(Fat32Error::Invalid("file has invalid first_cluster"));
        }

        info!(
            "FAT32: write_file_all '{}' size {} -> {} bytes (dirent lba=0x{:x} off={})",
            file.name, file.size, data.len(), file.dirent_lba, file.dirent_off
        );


        // Compute chain capacity in bytes by walking the chain and counting clusters.
        let spc = self.bpb.sec_per_clus as u64;
        let bytes_per_cluster = (spc as usize) * SECTOR_SIZE;

        // Count clusters (bounded)
        let mut chain_clusters: u32 = 0;
        {
            let mut cur = file.first_cluster;

            // Worst-case number of clusters in volume (guard against corruption)
            let max_steps = (self.bpb.tot_sec as u64 / self.bpb.sec_per_clus as u64).max(1);

            for _ in 0..max_steps {
                chain_clusters = chain_clusters.wrapping_add(1);

                let link = self.fat32_get_link(dev, cur)?;
                match link {
                    FatLink::Next(n) => {
                        if n == cur {
                            return Err(Fat32Error::Invalid("FAT self-loop in file chain"));
                        }
                        cur = n;
                    }
                    FatLink::Eoc => break,
                    FatLink::Bad => return Err(Fat32Error::Invalid("BAD cluster in file chain")),
                    FatLink::Free => return Err(Fat32Error::Invalid("FREE cluster in file chain")),
                    FatLink::Reserved(_) => break, // conservative
                }
            }
        }

        let capacity_bytes = (chain_clusters as usize) * bytes_per_cluster;

        if data.len() > capacity_bytes {
            return Err(Fat32Error::Invalid(
                "write exceeds allocated cluster chain (no allocation implemented yet)",
            ));
        }

        // Write file data to sectors (zero-fill tail of last sector we touch).
        let mut pos = 0usize;
        let mut cur = file.first_cluster;

        let max_steps = (self.bpb.tot_sec as u64 / self.bpb.sec_per_clus as u64).max(1);

        for _ in 0..max_steps {
            if pos >= data.len() {
                break;
            }

            let base_lba = self.cluster_to_lba(cur)?;
            for s in 0..spc {
                if pos >= data.len() {
                    break;
                }

                let mut sec = [0u8; SECTOR_SIZE];
                let remain = data.len() - pos;
                let take = core::cmp::min(SECTOR_SIZE, remain);
                sec[..take].copy_from_slice(&data[pos..pos + take]);

                dev.write_sector(base_lba + s, &sec)
                    .map_err(Self::map_block_err)?;

                pos += take;
            }

            if pos >= data.len() {
                break;
            }

            match self.fat32_get_link(dev, cur)? {
                FatLink::Next(n) => {
                    if n == cur {
                        return Err(Fat32Error::Invalid("FAT self-loop in file chain"));
                    }
                    cur = n;
                }
                FatLink::Eoc => break,
                FatLink::Bad => return Err(Fat32Error::Invalid("BAD cluster in file chain")),
                FatLink::Free => return Err(Fat32Error::Invalid("FREE cluster in file chain")),
                FatLink::Reserved(_) => break,
            }
        }

        if pos != data.len() {
            return Err(Fat32Error::Invalid("short write: chain ended unexpectedly"));
        }

        // Update directory entry size (partition-relative sector).
        self.update_dirent_size(dev, file.dirent_lba, file.dirent_off, data.len() as u32)?;

        // Flush device (no-op for now, but keeps layering correct).
        dev.flush().map_err(Self::map_block_err)?;

        Ok(())
    }

    pub fn write_file_all_growing(
        &mut self,
        dev: &dyn BlockDevice,
        file: &FatFile,
        data: &[u8],
    ) -> Result<(), Fat32Error> {
        if file.first_cluster < 2 {
            return Err(Fat32Error::Invalid("file has invalid first_cluster"));
        }

        let bytes_per_cluster = self.bpb.sec_per_clus as usize * SECTOR_SIZE;
        let needed_clusters = core::cmp::max(
            1,
            (data.len() + bytes_per_cluster - 1) / bytes_per_cluster,
        );

        let mut chain = Vec::new();
        let mut cur = file.first_cluster;

        loop {
            chain.push(cur);

            match self.fat32_get_link(dev, cur)? {
                FatLink::Next(n) => cur = n,
                FatLink::Eoc => break,
                FatLink::Bad => return Err(Fat32Error::Invalid("BAD cluster in file chain")),
                FatLink::Free => return Err(Fat32Error::Invalid("FREE cluster in file chain")),
                FatLink::Reserved(_) => break,
            }
        }

        while chain.len() < needed_clusters {
            let new_cluster = self.fat32_alloc_cluster(dev)?;
            let last = *chain.last().unwrap();
            self.fat32_set_next(dev, last, new_cluster)?;
            self.fat32_set_eoc(dev, new_cluster)?;
            chain.push(new_cluster);
        }

        if chain.len() > needed_clusters {
            let first_extra = chain[needed_clusters];
            self.fat32_set_eoc(dev, chain[needed_clusters - 1])?;

            let mut cur = first_extra;
            loop {
                let next = self.fat32_get_link(dev, cur)?;
                self.fat32_set_free(dev, cur)?;

                match next {
                    FatLink::Next(n) => cur = n,
                    FatLink::Eoc => break,
                    _ => break,
                }
            }

            chain.truncate(needed_clusters);
        }

        let mut pos = 0usize;

        for &cluster in &chain {
            let base = self.cluster_to_lba(cluster)?;

            for s in 0..self.bpb.sec_per_clus as u64 {
                let mut sec = [0u8; SECTOR_SIZE];

                if pos < data.len() {
                    let take = core::cmp::min(SECTOR_SIZE, data.len() - pos);
                    sec[..take].copy_from_slice(&data[pos..pos + take]);
                    pos += take;
                }

                dev.write_sector(base + s, &sec)?;
            }
        }

        self.update_dirent_size(dev, file.dirent_lba, file.dirent_off, data.len() as u32)?;
        dev.flush().ok();

        Ok(())
    }

    fn update_dirent_size(
        &self,
        dev: &dyn BlockDevice,
        dirent_lba: u64,
        dirent_off: usize,
        new_size: u32,
    ) -> Result<(), Fat32Error> {
        if dirent_off + 32 > SECTOR_SIZE {
            return Err(Fat32Error::Invalid("dirent offset out of sector"));
        }
        if (dirent_off % 32) != 0 {
            return Err(Fat32Error::Invalid("dirent offset not aligned"));
        }

        let mut sector = [0u8; SECTOR_SIZE];
        dev.read_sector(dirent_lba, &mut sector)
            .map_err(Self::map_block_err)?;

        let _old_size = u32::from_le_bytes([
            sector[dirent_off + 28],
            sector[dirent_off + 29],
            sector[dirent_off + 30],
            sector[dirent_off + 31],
        ]);


        // Sanity: ensure entry is valid
        let first = sector[dirent_off];
        if first == 0x00 || first == 0xE5 {
            return Err(Fat32Error::Invalid("dirent no longer valid"));
        }

        // File size is at offset 28..31 in the 32-byte entry
        sector[dirent_off + 28..dirent_off + 32].copy_from_slice(&new_size.to_le_bytes());

        // Optionally set Archive bit on write (0x20)
        sector[dirent_off + 11] |= 0x20;

        dev.write_sector(dirent_lba, &sector)
            .map_err(Self::map_block_err)?;

        Ok(())
    }
}
