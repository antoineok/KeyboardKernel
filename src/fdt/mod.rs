use fdt::Fdt;
use heapless::LinearMap;

use crate::info;

/// One MMIO entry describing a block in the device tree.
#[derive(Clone, Copy, Debug)]
pub struct MmioEntry {
    /// VC MMIO address (0x7E000000 range)
    pub vc_addr: u64,

    /// ARM physical address (0xFE000000 range)
    pub arm_phys: u64,

    /// Region size in bytes
    pub size: u64,
}

/// Table of MMIO entries indexed by DT aliases like "uart0", "gpio", "spi1", etc.
#[derive(Debug)]
pub struct MmioTable<'a> {
    pub entries: LinearMap<&'a str, MmioEntry, 64>,
}

impl<'a> MmioTable<'a> {
    /// Build an MMIO table by scanning /aliases in the device tree.
    pub fn from_fdt(fdt: &Fdt<'a>) -> Self {
        let mut table = MmioTable {
            entries: LinearMap::new(),
        };

        const ALIAS_FILTER: &[&str] = &["phandle", "blconfig", "blpubkey", "soc"];

        // The /aliases node maps names ("gpio", "uart0") to real device paths.
        let aliases = fdt.find_node("/aliases").expect("No /aliases in DT");

        for prop in aliases.properties() {
            let alias = prop.name;

            // Skip reserved properties
            if ALIAS_FILTER.contains(&alias) || alias.starts_with('#') {
                continue;
            }

            // Read and parse the path string
            let raw = match core::str::from_utf8(prop.value) {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Trim at first NUL (DT strings are NUL-terminated)
            let path = match raw.split_once('\0') {
                Some((clean, _)) => clean,
                None => raw,
            };

            let node = match fdt.find_node(path) {
                Some(n) => n,
                None => continue,
            };

            // Get the first "reg" range
            // Get the first "reg" entry
            let mut regs = match node.reg() {
                Some(r) => r,
                None => {
                    // e.g. i2c0 pinmux nodes, leds, etc.
                    // log::info!("alias {alias} has no reg; skipping");
                    continue;
                }
            };

            let reg = match regs.next() {
                Some(r) => r,
                None => continue,
            };

            let vc = reg.starting_address as u64;
            let sz = reg.size.unwrap_or(0) as u64;

            // Only handle classic VC peripheral MMIO (0x7E00_0000 region) for now.
            // This covers things like gpio, uart0/1, aux, spi0/1/2, mailbox, etc.
            if vc < 0x7E00_0000 || vc >= 0x7F00_0000 {
                // e.g. reserved memory, SCB/PCIE, some i2c20/i2c21, etc.
                // log::info!("alias {alias} has non-VC base {:#x}; skipping", vc);
                continue;
            }

            // BCM2711: bus (VC) 0x7E00_0000 -> ARM phys 0xFE00_0000
            let arm = vc - 0x7E00_0000 + 0xFE00_0000;

            let entry = MmioEntry {
                vc_addr: vc,
                arm_phys: arm,
                size: sz,
            };

            // Ignore insertion failure if table is full / duplicate
            let _ = table.entries.insert(alias, entry);
        }

        table
    }

    /// Get an MMIO entry by alias name, e.g. "uart0" or "gpio".
    #[inline]
    pub fn get(&self, alias: &str) -> Option<MmioEntry> {
        self.entries.get(alias).copied()
    }
}
