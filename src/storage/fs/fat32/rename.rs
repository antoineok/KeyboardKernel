// storage/fs/fat32/rename.rs
extern crate alloc;

use alloc::vec::Vec;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

const ATTR_DIRECTORY: u8 = 0x10;

fn eq_ci(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.bytes()
        .zip(b.bytes())
        .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

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

    let name = comps.pop().ok_or(Fat32Error::Invalid("FAT32: empty path"))?;

    Ok((comps, name))
}

fn same_parent(a: &[&str], b: &[&str]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter().zip(b.iter()).all(|(x, y)| eq_ci(x, y))
}

impl Fat32Volume {

    pub fn rename_file_83(
        &self,
        dev: &dyn BlockDevice,
        old_path: &str,
        new_path: &str,
    ) -> Result<(), Fat32Error> {
        let (old_parent, old_name) = split_parent(old_path)?;
        let (new_parent, new_name) = split_parent(new_path)?;

        if !same_parent(&old_parent, &new_parent) {
            return Err(Fat32Error::Invalid(
                "FAT32: cross-directory rename not implemented",
            ));
        }

        let new_raw_name = make_short_name_83(new_name)?;

        let parent_cluster = self.resolve_dir_cluster_from_components(dev, &old_parent)?;

        let old_entry = self.find_in_dir(dev, parent_cluster, old_name)?;

        if (old_entry.attr & ATTR_DIRECTORY) != 0 {
            return Err(Fat32Error::Invalid(
                "FAT32: directory rename not implemented",
            ));
        }

        if self.find_in_dir(dev, parent_cluster, new_name).is_ok() {
            return Err(Fat32Error::Invalid(
                "FAT32: rename target already exists",
            ));
        }

        if old_entry.dirent_off + 32 > SECTOR_SIZE {
            return Err(Fat32Error::Invalid(
                "FAT32: invalid directory entry offset",
            ));
        }

        let mut sector = [0u8; SECTOR_SIZE];

        dev.read_sector(old_entry.dirent_lba, &mut sector)?;

        let entry = &mut sector[old_entry.dirent_off..old_entry.dirent_off + 32];

        entry[0..11].copy_from_slice(&new_raw_name);

        // Clear NT lowercase flags. Not required, but keeps the renamed 8.3 entry simple.
        entry[12] = 0;

        dev.write_sector(old_entry.dirent_lba, &sector)?;
        dev.flush().ok();

        Ok(())
    }
}