use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::fs::fat32::fat::FatLink;
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};

const ATTR_DIRECTORY: u8 = 0x10;

impl Fat32Volume {
    pub fn delete_file_83(
        &self,
        dev: &dyn BlockDevice,
        path: &str,
    ) -> Result<(), Fat32Error> {
        let entry = self.resolve_path(dev, path)?;

        if (entry.attr & ATTR_DIRECTORY) != 0 {
            return Err(Fat32Error::Invalid(
                "FAT32: directory deletion not implemented",
            ));
        }

        if entry.dirent_off + 32 > SECTOR_SIZE {
            return Err(Fat32Error::Invalid(
                "FAT32: invalid directory entry offset",
            ));
        }

        if entry.first_cluster >= 2 {
            self.free_cluster_chain(dev, entry.first_cluster)?;
        }

        let mut sector = [0u8; SECTOR_SIZE];

        dev.read_sector(entry.dirent_lba, &mut sector)?;

        sector[entry.dirent_off] = 0xE5;

        dev.write_sector(entry.dirent_lba, &sector)?;
        dev.flush()?;

        Ok(())
    }

    pub fn free_cluster_chain(
        &self,
        dev: &dyn BlockDevice,
        start: u32,
    ) -> Result<(), Fat32Error> {
        if start < 2 {
            return Err(Fat32Error::Invalid("FAT32: invalid cluster chain start"));
        }

        let mut cur = start;

        let max_steps = (self.bpb.tot_sec as u64 / self.bpb.sec_per_clus as u64).max(1);

        for _ in 0..max_steps {
            let next = self.fat32_get_link(dev, cur)?;

            self.fat32_set_free(dev, cur)?;

            match next {
                FatLink::Next(n) => {
                    if n == cur {
                        return Err(Fat32Error::Invalid("FAT32: self-loop in cluster chain"));
                    }

                    cur = n;
                }
                FatLink::Eoc => return Ok(()),
                FatLink::Free => return Ok(()),
                FatLink::Bad => {
                    return Err(Fat32Error::Invalid("FAT32: BAD cluster in delete chain"));
                }
                FatLink::Reserved(_) => return Ok(()),
            }
        }

        Err(Fat32Error::Invalid(
            "FAT32: cluster chain too long while deleting",
        ))
    }
}