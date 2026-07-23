extern crate alloc;

use crate::storage::block::{BlockDevice, SECTOR_SIZE};
use crate::storage::fs::fat32::{Fat32Error, Fat32Volume};
use crate::{info, warn};
use alloc::string::String;

#[derive(Clone, Copy, Debug)]
pub struct DirEntry83 {
    pub name_raw: [u8; 11],
    pub attr: u8,
    pub first_cluster: u32,
    pub file_size: u32,
}

#[inline(always)]
pub(super) fn is_lfn(attr: u8) -> bool {
    attr == 0x0F
}

fn trim_ascii(bytes: &[u8]) -> &str {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    core::str::from_utf8(&bytes[..end]).unwrap_or("<non-utf8>")
}

pub(super) fn short_name_to_string(name_raw: &[u8; 11]) -> String {
    let name = &name_raw[0..8];
    let ext = &name_raw[8..11];

    let name_s = trim_ascii(name);
    let ext_s = trim_ascii(ext);

    if !ext_s.is_empty() {
        let mut s = String::new();
        s.push_str(name_s);
        s.push('.');
        s.push_str(ext_s);
        s
    } else {
        String::from(name_s)
    }
}

/// LFN checksum of the 8.3 short name (11 bytes).
/// Standard algorithm used by VFAT.
fn lfn_checksum(short_name_11: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &b in short_name_11.iter() {
        // sum = ((sum & 1) << 7) + (sum >> 1) + b
        sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(b);
    }
    sum
}

/// Extract the 13 UTF-16LE code units from an LFN entry.
/// Layout:
/// - name1: bytes 1..10  (5 u16)
/// - name2: bytes 14..25 (6 u16)
/// - name3: bytes 28..31 (2 u16)
fn lfn_entry_name_u16(entry: &[u8]) -> [u16; 13] {
    let mut out = [0u16; 13];

    // Helper to read u16 LE from two bytes
    #[inline(always)]
    fn u16le(a: u8, b: u8) -> u16 {
        (a as u16) | ((b as u16) << 8)
    }

    // name1 (5)
    for i in 0..5 {
        let o = 1 + i * 2;
        out[i] = u16le(entry[o], entry[o + 1]);
    }

    // name2 (6)
    for i in 0..6 {
        let o = 14 + i * 2;
        out[5 + i] = u16le(entry[o], entry[o + 1]);
    }

    // name3 (2)
    for i in 0..2 {
        let o = 28 + i * 2;
        out[11 + i] = u16le(entry[o], entry[o + 1]);
    }

    out
}

/// Convert UTF-16 code units (with 0x0000 terminator and 0xFFFF padding) into a Rust String.
/// This is a best-effort decoder:
/// - stops at 0x0000
/// - ignores 0xFFFF
/// - supports BMP and surrogate pairs
fn utf16_units_to_string(units: &[u16]) -> String {
    let mut s = String::new();
    let mut i = 0;

    while i < units.len() {
        let u = units[i];

        if u == 0x0000 {
            break;
        }
        if u == 0xFFFF {
            i += 1;
            continue;
        }

        // Surrogate pair handling
        if (0xD800..=0xDBFF).contains(&u) {
            if i + 1 < units.len() {
                let u2 = units[i + 1];
                if (0xDC00..=0xDFFF).contains(&u2) {
                    let high = (u as u32) - 0xD800;
                    let low = (u2 as u32) - 0xDC00;
                    let codepoint = 0x10000 + ((high << 10) | low);
                    if let Some(ch) = char::from_u32(codepoint) {
                        s.push(ch);
                    } else {
                        s.push('\u{FFFD}');
                    }
                    i += 2;
                    continue;
                }
            }
            // Invalid surrogate
            s.push('\u{FFFD}');
            i += 1;
            continue;
        }

        if let Some(ch) = char::from_u32(u as u32) {
            s.push(ch);
        } else {
            s.push('\u{FFFD}');
        }

        i += 1;
    }

    s
}

/// Max VFAT long filename length is 255 UTF-16 code units.
/// Keep a bit of slack.
const LFN_MAX_UNITS: usize = 260;

pub(super) struct LfnState {
    active: bool,
    checksum: u8,
    last_ord: u8,     // total number of LFN entries in this chain
    expected_ord: u8, // next expected ord (descending)
    buf: [u16; LFN_MAX_UNITS],
}

impl LfnState {
    pub(super) const fn new() -> Self {
        Self {
            active: false,
            checksum: 0,
            last_ord: 0,
            expected_ord: 0,
            buf: [0u16; LFN_MAX_UNITS],
        }
    }

    pub(super) fn reset(&mut self) {
        self.active = false;
        self.checksum = 0;
        self.last_ord = 0;
        self.expected_ord = 0;
        // No need to clear buf; we’ll overwrite the used region.
    }

    pub(super) fn start(&mut self, last_ord: u8, checksum: u8) -> bool {
        // last_ord * 13 must fit in our buffer
        let needed = (last_ord as usize) * 13;
        if last_ord == 0 || needed > LFN_MAX_UNITS {
            self.reset();
            return false;
        }

        self.active = true;
        self.checksum = checksum;
        self.last_ord = last_ord;
        self.expected_ord = last_ord;
        true
    }

    /// Feed one LFN entry. Returns false if the chain is invalid and was reset.
    pub(super) fn push_entry(&mut self, entry: &[u8]) -> bool {
        let seq = entry[0];
        let ord = seq & 0x1F;
        let is_last = (seq & 0x40) != 0;
        let chk = entry[13];

        // If we see a "LAST" entry while already active, treat it as a new chain start.
        // But do it safely: reset first.
        if is_last {
            self.reset();
            if !self.start(ord, chk) {
                return false;
            }
        } else {
            // Must already be in a chain
            if !self.active {
                return false;
            }
            // checksum must match
            if chk != self.checksum {
                self.reset();
                return false;
            }
        }

        // Now chain must be active
        if !self.active {
            return false;
        }

        // Validate order sequence (descending)
        if ord != self.expected_ord || ord == 0 || ord > self.last_ord {
            self.reset();
            return false;
        }

        // Extract 13 UTF-16 units and place them at the correct offset
        let frag = lfn_entry_name_u16(entry);
        let base = (ord as usize - 1) * 13;
        if base + 13 > LFN_MAX_UNITS {
            self.reset();
            return false;
        }
        self.buf[base..base + 13].copy_from_slice(&frag);

        // Next expected
        if self.expected_ord == 1 {
            // Completed chain; hold active until consumed by the following short entry
            self.expected_ord = 0;
        } else {
            self.expected_ord -= 1;
        }

        true
    }

    pub(super) fn consume_if_matches(&mut self, short_name: &[u8; 11]) -> Option<String> {
        if !self.active {
            return None;
        }
        // Must have finished sequence down to 1 (expected_ord==0)
        if self.expected_ord != 0 {
            self.reset();
            return None;
        }

        let chk = lfn_checksum(short_name);
        if chk != self.checksum {
            self.reset();
            return None;
        }

        let used = (self.last_ord as usize) * 13;
        let s = utf16_units_to_string(&self.buf[..used]);

        self.reset();
        Some(s)
    }
}

/// List root directory entries, now with LFN support.
/// For now, still assumes root directory fits in one cluster. (Next step: FAT chaining)
pub fn list_root_dir(dev: &dyn BlockDevice, vol: &Fat32Volume) -> Result<(), Fat32Error> {
    info!(
        "FAT32: listing root directory (cluster {})",
        vol.bpb.root_clus
    );

    let mut lfn = LfnState::new();

    // We'll stop early if we hit 0x00 (no more entries).
    let mut done = false;

    vol.follow_cluster_chain(dev, vol.bpb.root_clus, |cluster| {
        if done {
            return Ok(());
        }

        let base_lba = vol.cluster_to_lba(cluster)?;
        let spc = vol.bpb.sec_per_clus as u64;

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

                if is_lfn(attr) {
                    let _ = lfn.push_entry(entry);
                    continue;
                }

                let mut name_raw = [0u8; 11];
                name_raw.copy_from_slice(&entry[0..11]);

                let long_name = lfn.consume_if_matches(&name_raw);
                let short_name = short_name_to_string(&name_raw);

                // Cluster high/low
                let clus_hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
                let clus_lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
                let first_cluster = (clus_hi << 16) | clus_lo;

                let file_size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);

                // Skip volume label entries in normal listings (attr bit 0x08)
                if (attr & 0x08) != 0 {
                    continue;
                }

                info!("  Entry:");
                if let Some(lfn_name) = &long_name {
                    info!("    name: {} (short: {})", lfn_name, short_name);
                } else {
                    info!("    name: {}", short_name);
                }
                info!(
                    "    attr: 0x{:02x} cluster: {} size: {}",
                    attr, first_cluster, file_size
                );
            }
        }

        Ok(())
    })?;

    if !done {
        warn!("FAT32: root dir chain ended (EOC) without 0x00 terminator (still OK on some media)");
    }

    Ok(())
}
