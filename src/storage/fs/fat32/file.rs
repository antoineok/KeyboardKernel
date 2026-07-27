extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::info;
use crate::storage::block::{BlockDevice, BlockError, SECTOR_SIZE};
use crate::storage::fs::fat32::fat::FatLink;
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

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

/// Sequential, allocation-free reader for a FAT file.
///
/// The cursor retains its current cluster and a one-sector cache, so repeated
/// small reads do not restart the FAT traversal or reread a partially consumed
/// sector.
pub struct FatFileReader {
    file: FatFile,
    position: usize,
    current_cluster: u32,
    cluster_offset: usize,
    cached_lba: Option<u64>,
    sector: [u8; SECTOR_SIZE],
}

impl FatFileReader {
    pub fn new(file: FatFile) -> Result<Self, Fat32Error> {
        if file.size != 0 && file.first_cluster < 2 {
            return Err(Fat32Error::Invalid("file has invalid first_cluster"));
        }

        let current_cluster = file.first_cluster;

        Ok(Self {
            file,
            position: 0,
            current_cluster,
            cluster_offset: 0,
            cached_lba: None,
            sector: [0; SECTOR_SIZE],
        })
    }

    #[inline(always)]
    pub fn name(&self) -> &str {
        &self.file.name
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.file.size as usize
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.file.size == 0
    }

    #[inline(always)]
    pub fn position(&self) -> usize {
        self.position
    }

    #[inline(always)]
    pub fn remaining(&self) -> usize {
        self.len().saturating_sub(self.position)
    }
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

    /// Create a sequential reader for an already resolved file.
    pub fn open_file_reader(&self, file: FatFile) -> Result<FatFileReader, Fat32Error> {
        FatFileReader::new(file)
    }

    /// Reposition a sequential file reader without reading file payload bytes.
    ///
    /// FAT does not provide byte offsets directly, so seeking resolves the
    /// cluster containing the requested position from the start of the file.
    /// This is still much cheaper than replaying every data sector when a
    /// streamed media asset loops or resumes after its RAM bootstrap cache.
    pub fn seek_file(
        &self,
        dev: &dyn BlockDevice,
        reader: &mut FatFileReader,
        position: usize,
    ) -> Result<(), Fat32Error> {
        if position > reader.len() {
            return Err(Fat32Error::Invalid("file seek exceeds declared size"));
        }

        if reader.is_empty() {
            reader.position = 0;
            reader.current_cluster = reader.file.first_cluster;
            reader.cluster_offset = 0;
            reader.cached_lba = None;
            return Ok(());
        }

        let bytes_per_cluster = (self.bpb.sec_per_clus as usize)
            .checked_mul(SECTOR_SIZE)
            .ok_or(Fat32Error::Invalid("FAT32 cluster size overflow"))?;

        if bytes_per_cluster == 0 {
            return Err(Fat32Error::Invalid("FAT32 cluster size is zero"));
        }

        // At EOF on an exact cluster boundary, keep the cursor on the last
        // cluster with cluster_offset == bytes_per_cluster. read_file() checks
        // EOF before attempting to advance to a non-existent next cluster.
        let (cluster_index, cluster_offset) = if position == reader.len() {
            let last_byte = position - 1;
            (
                last_byte / bytes_per_cluster,
                (last_byte % bytes_per_cluster) + 1,
            )
        } else {
            (position / bytes_per_cluster, position % bytes_per_cluster)
        };

        let mut current_cluster = reader.file.first_cluster;

        for _ in 0..cluster_index {
            current_cluster = match self.fat32_get_link(dev, current_cluster)? {
                FatLink::Next(next) => {
                    if next == current_cluster {
                        return Err(Fat32Error::Invalid("FAT self-loop in file chain"));
                    }
                    next
                }
                FatLink::Eoc => {
                    return Err(Fat32Error::Invalid(
                        "file cluster chain ended before seek position",
                    ));
                }
                FatLink::Bad => return Err(Fat32Error::Invalid("BAD cluster in file chain")),
                FatLink::Free => return Err(Fat32Error::Invalid("FREE cluster in file chain")),
                FatLink::Reserved(_) => {
                    return Err(Fat32Error::Invalid("reserved cluster in file chain"));
                }
            };
        }

        reader.position = position;
        reader.current_cluster = current_cluster;
        reader.cluster_offset = cluster_offset;
        reader.cached_lba = None;

        Ok(())
    }

    /// Continue reading a file from the reader's current position.
    ///
    /// Returns the number of bytes copied into `out`. A return value of zero
    /// means either `out` is empty or the reader reached the declared file
    /// length.
    pub fn read_file(
        &self,
        dev: &dyn BlockDevice,
        reader: &mut FatFileReader,
        out: &mut [u8],
    ) -> Result<usize, Fat32Error> {
        if out.is_empty() || reader.position >= reader.len() {
            return Ok(0);
        }

        let bytes_per_cluster = (self.bpb.sec_per_clus as usize)
            .checked_mul(SECTOR_SIZE)
            .ok_or(Fat32Error::Invalid("FAT32 cluster size overflow"))?;

        if bytes_per_cluster == 0 {
            return Err(Fat32Error::Invalid("FAT32 cluster size is zero"));
        }

        let requested = core::cmp::min(out.len(), reader.remaining());
        let mut written = 0usize;

        while written < requested {
            if reader.cluster_offset == bytes_per_cluster {
                reader.current_cluster = match self.fat32_get_link(dev, reader.current_cluster)? {
                    FatLink::Next(next) => {
                        if next == reader.current_cluster {
                            return Err(Fat32Error::Invalid("FAT self-loop in file chain"));
                        }

                        next
                    }
                    FatLink::Eoc => {
                        return Err(Fat32Error::Invalid(
                            "file cluster chain ended before declared size",
                        ));
                    }
                    FatLink::Bad => {
                        return Err(Fat32Error::Invalid("BAD cluster in file chain"));
                    }
                    FatLink::Free => {
                        return Err(Fat32Error::Invalid("FREE cluster in file chain"));
                    }
                    FatLink::Reserved(_) => {
                        return Err(Fat32Error::Invalid("reserved cluster in file chain"));
                    }
                };

                reader.cluster_offset = 0;
            }

            let sector_index = reader.cluster_offset / SECTOR_SIZE;
            let sector_offset = reader.cluster_offset % SECTOR_SIZE;
            let base_lba = self.cluster_to_lba(reader.current_cluster)?;
            let lba = base_lba + sector_index as u64;

            if reader.cached_lba != Some(lba) {
                dev.read_sector(lba, &mut reader.sector)
                    .map_err(Self::map_block_err)?;
                reader.cached_lba = Some(lba);
            }

            let available = SECTOR_SIZE - sector_offset;
            let take = core::cmp::min(requested - written, available);

            out[written..written + take]
                .copy_from_slice(&reader.sector[sector_offset..sector_offset + take]);

            reader.position += take;
            reader.cluster_offset += take;
            written += take;
        }

        Ok(written)
    }

    /// Read up to `out.len()` bytes from a file, starting at offset 0.
    /// Returns number of bytes written to `out`.
    pub fn read_file_prefix(
        &self,
        dev: &dyn BlockDevice,
        file: &FatFile,
        out: &mut [u8],
    ) -> Result<usize, Fat32Error> {
        let mut reader = self.open_file_reader(file.clone())?;
        self.read_file(dev, &mut reader, out)
    }

    pub fn exists(&self, dev: &dyn BlockDevice, path: &str) -> bool {
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
            file.name,
            file.size,
            data.len(),
            file.dirent_lba,
            file.dirent_off
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
        let needed_clusters =
            core::cmp::max(1, (data.len() + bytes_per_cluster - 1) / bytes_per_cluster);

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
