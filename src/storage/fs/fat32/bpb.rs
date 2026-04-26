use crate::storage::block::SECTOR_SIZE;
use crate::storage::fs::fat32::Fat32Error;

/// Minimal FAT32 BPB fields we need initially.
#[derive(Clone, Copy, Debug)]
pub struct Fat32Bpb {
    pub bytes_per_sec: u16,
    pub sec_per_clus: u8,
    pub rsvd_sec_cnt: u16,
    pub num_fats: u8,
    pub tot_sec: u32,
    pub fat_sz: u32,
    pub root_clus: u32,

    // For convenience/debug
    pub vol_label: [u8; 11],
    pub oem: [u8; 8],
}

impl Fat32Bpb {
    pub fn parse(vbr: &[u8; SECTOR_SIZE]) -> Result<Self, Fat32Error> {
        // Basic boot sector signature
        let sig = (vbr[510] as u16) | ((vbr[511] as u16) << 8);
        if sig != 0xAA55 {
            return Err(Fat32Error::Invalid("VBR signature != 0xAA55"));
        }

        let bytes_per_sec = u16::from_le_bytes([vbr[0x0B], vbr[0x0C]]);
        let sec_per_clus = vbr[0x0D];
        let rsvd_sec_cnt = u16::from_le_bytes([vbr[0x0E], vbr[0x0F]]);
        let num_fats = vbr[0x10];

        let tot_sec_16 = u16::from_le_bytes([vbr[0x13], vbr[0x14]]) as u32;
        let tot_sec_32 = u32::from_le_bytes([vbr[0x20], vbr[0x21], vbr[0x22], vbr[0x23]]);
        let tot_sec = if tot_sec_16 != 0 { tot_sec_16 } else { tot_sec_32 };

        let fat_sz_16 = u16::from_le_bytes([vbr[0x16], vbr[0x17]]) as u32;
        let fat_sz_32 = u32::from_le_bytes([vbr[0x24], vbr[0x25], vbr[0x26], vbr[0x27]]);
        let fat_sz = if fat_sz_16 != 0 { fat_sz_16 } else { fat_sz_32 };

        let root_clus = u32::from_le_bytes([vbr[0x2C], vbr[0x2D], vbr[0x2E], vbr[0x2F]]);

        let mut vol_label = [0u8; 11];
        vol_label.copy_from_slice(&vbr[0x47..0x47 + 11]);

        let mut oem = [0u8; 8];
        oem.copy_from_slice(&vbr[0x03..0x03 + 8]);

        // Sanity checks for your current system
        if bytes_per_sec as usize != SECTOR_SIZE {
            return Err(Fat32Error::Invalid("bytes_per_sec != 512"));
        }
        if sec_per_clus == 0 {
            return Err(Fat32Error::Invalid("sec_per_clus == 0"));
        }
        if rsvd_sec_cnt == 0 {
            return Err(Fat32Error::Invalid("rsvd_sec_cnt == 0"));
        }
        if num_fats == 0 {
            return Err(Fat32Error::Invalid("num_fats == 0"));
        }
        if fat_sz == 0 {
            return Err(Fat32Error::Invalid("fat_sz == 0"));
        }
        if root_clus < 2 {
            return Err(Fat32Error::Invalid("root_clus < 2"));
        }
        if tot_sec == 0 {
            return Err(Fat32Error::Invalid("tot_sec == 0"));
        }

        Ok(Self {
            bytes_per_sec,
            sec_per_clus,
            rsvd_sec_cnt,
            num_fats,
            tot_sec,
            fat_sz,
            root_clus,
            vol_label,
            oem,
        })
    }
}
