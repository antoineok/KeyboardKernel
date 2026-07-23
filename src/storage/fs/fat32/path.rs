extern crate alloc;

use alloc::string::String;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

/// FAT directory attribute flags we care about.
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_VOLUME_ID: u8 = 0x08;

/// A directory entry resolved by name lookup (long or short).
#[derive(Clone, Debug)]
pub struct FoundEntry {
    pub name: String,       // long name if present, else short
    pub short_name: String, // always present for debug
    pub attr: u8,
    pub first_cluster: u32,
    pub size: u32,
    pub dirent_lba: u64,
    pub dirent_off: usize,
}

impl FoundEntry {
    pub fn is_dir(&self) -> bool {
        (self.attr & ATTR_DIRECTORY) != 0
    }
}

/// Case-insensitive ASCII compare (VFAT-style).
fn eq_ci(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

/// Split a path into components.
/// - Accepts "a/b", "/a/b", "a//b", etc.
/// - Ignores empty and "." components
/// - Rejects ".." for now (safer)
fn split_path(path: &str) -> Result<alloc::vec::Vec<&str>, Fat32Error> {
    let mut out = alloc::vec::Vec::new();

    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(Fat32Error::Invalid("FAT32: '..' not supported in paths"));
        }
        out.push(comp);
    }

    Ok(out)
}

impl Fat32Volume {
    /// Scan a directory cluster chain and find a matching entry by name (LFN or short).
    pub fn find_in_dir(
        &self,
        dev: &dyn BlockDevice,
        dir_cluster: u32,
        name: &str,
    ) -> Result<FoundEntry, Fat32Error> {
        if dir_cluster < 2 {
            return Err(Fat32Error::Invalid("FAT32: dir_cluster < 2"));
        }

        let mut lfn = super::dir::LfnState::new();
        let mut found: Option<FoundEntry> = None;
        let mut done = false;

        self.follow_cluster_chain(dev, dir_cluster, |cluster| {
            if done || found.is_some() {
                return Ok(());
            }

            let base_lba = self.cluster_to_lba(cluster)?;
            let spc = self.bpb.sec_per_clus as u64;

            for s in 0..spc {
                let mut sector = [0u8; SECTOR_SIZE];
                dev.read_sector(base_lba + s, &mut sector)?;

                for e in 0..(SECTOR_SIZE / 32) {
                    let off = e * 32;
                    let entry = &sector[off..off + 32];
                    let first = entry[0];

                    if first == 0x00 {
                        done = true;
                        return Ok(());
                    }
                    if first == 0xE5 {
                        lfn.reset();
                        continue;
                    }

                    let attr = entry[11];

                    if super::dir::is_lfn(attr) {
                        let _ = lfn.push_entry(entry);
                        continue;
                    }

                    // Short name raw
                    let mut name_raw = [0u8; 11];
                    name_raw.copy_from_slice(&entry[0..11]);

                    let long_name = lfn.consume_if_matches(&name_raw);
                    let short_name = super::dir::short_name_to_string(&name_raw);

                    // Skip volume label entries
                    if (attr & ATTR_VOLUME_ID) != 0 {
                        continue;
                    }

                    // cluster hi/lo
                    let clus_hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
                    let clus_lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
                    let first_cluster = (clus_hi << 16) | clus_lo;

                    let size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);

                    let dirent_lba = (base_lba + s) as u64;
                    let dirent_off = off;

                    // Match rules: prefer long name if present, else short.
                    let matched = if let Some(lfn_name) = &long_name {
                        eq_ci(lfn_name, name)
                    } else {
                        eq_ci(&short_name, name)
                    };

                    if matched {
                        found = Some(FoundEntry {
                            name: long_name.clone().unwrap_or_else(|| short_name.clone()),
                            short_name,
                            attr,
                            first_cluster,
                            size,
                            dirent_lba,
                            dirent_off,
                        });
                        return Ok(());
                    }
                }
            }

            Ok(())
        })?;

        found.ok_or(Fat32Error::Invalid("FAT32: entry not found"))
    }

    /// Resolve a path starting from root directory and return the final entry.
    /// Works for files or directories.
    pub fn resolve_path(
        &self,
        dev: &dyn BlockDevice,
        path: &str,
    ) -> Result<FoundEntry, Fat32Error> {
        let comps = split_path(path)?;
        if comps.is_empty() {
            return Err(Fat32Error::Invalid("FAT32: empty path"));
        }

        let mut cur_dir = self.bpb.root_clus;
        let mut last: Option<FoundEntry> = None;

        for (i, comp) in comps.iter().enumerate() {
            let e = self.find_in_dir(dev, cur_dir, comp)?;
            let is_last = i + 1 == comps.len();

            if !is_last {
                if !e.is_dir() {
                    return Err(Fat32Error::Invalid(
                        "FAT32: path component is not a directory",
                    ));
                }
                // Directory entries can have first_cluster = 0 for root in some cases,
                // but for subdirs it should be >=2.
                if e.first_cluster < 2 {
                    return Err(Fat32Error::Invalid(
                        "FAT32: directory has invalid first_cluster",
                    ));
                }
                cur_dir = e.first_cluster;
            }

            last = Some(e);
        }

        last.ok_or(Fat32Error::Invalid("FAT32: resolve_path failed"))
    }

    /// Convenience: open a file by path (root-relative).
    /// Errors if the final entry is a directory.
    pub fn open_file_path(
        &self,
        dev: &dyn BlockDevice,
        path: &str,
    ) -> Result<super::file::FatFile, Fat32Error> {
        let e = self.resolve_path(dev, path)?;

        if e.is_dir() {
            return Err(Fat32Error::Invalid("FAT32: path is a directory"));
        }
        if e.first_cluster < 2 {
            return Err(Fat32Error::Invalid("FAT32: file has invalid first_cluster"));
        }

        Ok(super::file::FatFile {
            name: e.name,
            first_cluster: e.first_cluster,
            size: e.size,
            dirent_lba: e.dirent_lba,
            dirent_off: e.dirent_off,
        })
    }
}
