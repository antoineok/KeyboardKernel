// storage/fs/fat32/create.rs
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::fs::fat32::file::FatFile;
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

const ATTR_ARCHIVE: u8 = 0x20;

fn make_short_name_83(name: &str) -> Result<[u8; 11], Fat32Error> {
    let mut out = [b' '; 11];

    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };

    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(Fat32Error::Invalid("FAT32: not an 8.3 filename"));
    }

    for (i, b) in base.bytes().enumerate() {
        if !b.is_ascii_alphanumeric() && b != b'_' && b != b'-' {
            return Err(Fat32Error::Invalid("FAT32: invalid 8.3 character"));
        }
        out[i] = b.to_ascii_uppercase();
    }

    for (i, b) in ext.bytes().enumerate() {
        if !b.is_ascii_alphanumeric() && b != b'_' && b != b'-' {
            return Err(Fat32Error::Invalid("FAT32: invalid 8.3 extension character"));
        }
        out[8 + i] = b.to_ascii_uppercase();
    }

    Ok(out)
}

fn split_parent(path: &str) -> Result<(Vec<&str>, &str), Fat32Error> {
    let mut comps = Vec::new();

    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(Fat32Error::Invalid("FAT32: '..' not supported"));
        }
        comps.push(comp);
    }

    let file_name = comps.pop().ok_or(Fat32Error::Invalid("FAT32: empty path"))?;
    Ok((comps, file_name))
}

impl Fat32Volume {
    pub(crate) fn resolve_dir_cluster_from_components(
        &self,
        dev: &dyn BlockDevice,
        comps: &[&str],
    ) -> Result<u32, Fat32Error> {
        let mut cur = self.bpb.root_clus;

        for comp in comps {
            let e = self.find_in_dir(dev, cur, comp)?;
            if !e.is_dir() {
                return Err(Fat32Error::Invalid("FAT32: parent component is not directory"));
            }
            if e.first_cluster < 2 {
                return Err(Fat32Error::Invalid("FAT32: invalid directory cluster"));
            }
            cur = e.first_cluster;
        }

        Ok(cur)
    }

    fn find_free_dirent_slot(
        &self,
        dev: &dyn BlockDevice,
        dir_cluster: u32,
    ) -> Result<(u64, usize), Fat32Error> {
        let mut result = None;

        self.follow_cluster_chain(dev, dir_cluster, |cluster| {
            if result.is_some() {
                return Ok(());
            }

            let base = self.cluster_to_lba(cluster)?;

            for s in 0..self.bpb.sec_per_clus as u64 {
                let lba = base + s;
                let mut sec = [0u8; SECTOR_SIZE];
                dev.read_sector(lba, &mut sec)?;

                for e in 0..SECTOR_SIZE / 32 {
                    let off = e * 32;
                    let first = sec[off];

                    if first == 0x00 || first == 0xE5 {
                        result = Some((lba, off));
                        return Ok(());
                    }
                }
            }

            Ok(())
        })?;

        result.ok_or(Fat32Error::Invalid(
            "FAT32: no free dirent slot; directory growth not implemented",
        ))
    }

    pub fn create_file_83(
        &mut self,
        dev: &dyn BlockDevice,
        path: &str,
    ) -> Result<FatFile, Fat32Error> {
        let (parent_comps, file_name) = split_parent(path)?;
        let short = make_short_name_83(file_name)?;

        let parent_cluster = self.resolve_dir_cluster_from_components(dev, &parent_comps)?;

        if self.find_in_dir(dev, parent_cluster, file_name).is_ok() {
            return Err(Fat32Error::Invalid("FAT32: file already exists"));
        }

        let first_cluster = self.fat32_alloc_cluster(dev)?;
        let (dirent_lba, dirent_off) = self.find_free_dirent_slot(dev, parent_cluster)?;

        let mut sec = [0u8; SECTOR_SIZE];
        dev.read_sector(dirent_lba, &mut sec)?;

        let e = &mut sec[dirent_off..dirent_off + 32];
        e.fill(0);

        e[0..11].copy_from_slice(&short);
        e[11] = ATTR_ARCHIVE;

        let hi = ((first_cluster >> 16) as u16).to_le_bytes();
        let lo = ((first_cluster & 0xFFFF) as u16).to_le_bytes();

        e[20..22].copy_from_slice(&hi);
        e[26..28].copy_from_slice(&lo);
        e[28..32].copy_from_slice(&0u32.to_le_bytes());

        dev.write_sector(dirent_lba, &sec)?;

        dev.flush().ok();

        Ok(FatFile {
            name: String::from(file_name),
            first_cluster,
            size: 0,
            dirent_lba,
            dirent_off,
        })
    }
}