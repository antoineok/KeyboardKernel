use fdt::Fdt;
use core::fmt::Debug;
/// One MMIO entry describing a block in the device tree.
#[derive(Clone, Copy)]
pub struct AliasEntry {
    /// VC MMIO address (0x7E000000 range)
    pub vc_addr: u64,

    /// ARM physical address (0xFE000000 range)
    pub arm_phys: u64,

    /// Region size in bytes
    pub size: u64,
}

impl Debug for AliasEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "AliasEntry {{ vc_addr: 0x{:x}, arm_phys: 0x{:x}, size: 0x{:x} }}",
            self.vc_addr, self.arm_phys, self.size
        )
    }
}

/// Table of MMIO entries indexed by DT aliases like "uart0", "gpio", "spi1", etc.
#[derive(Debug)]
pub struct AliasesTable<'a> {
    fdt: &'a Fdt<'a>,
}

impl<'a> AliasesTable<'a> {
    /// Build an MMIO table by scanning /aliases in the device tree.
    pub fn new(fdt: &'a Fdt<'a>) -> Self {
        AliasesTable { fdt }
    }

    /// Get an MMIO entry by alias name, e.g. "uart0" or "gpio".
    #[inline]
    pub fn get(&self, alias: &str) -> Option<AliasEntry> {
        let aliases = match self.fdt.aliases() {
            Some(a) => a,
            None => {
                return None;
            }
        };

        let node = match aliases.resolve_node(alias) {
            Some(n) => n,
            None => {
                return None;
            }
        };

        let mut regs = match node.reg() {
            Some(r) => r,
            None => {
                return None;
            }
        };

        let reg = match regs.next() {
            Some(r) => r,
            None => {
                return None;
            }
        };
        let vc = reg.starting_address as u64;
        let sz = reg.size.unwrap_or(0) as u64; // Only handle classic VC peripheral MMIO (0x7E00_0000 region) for now.
        if !((0x7E00_0000..0x7F00_0000).contains(&vc)) {
            return None;
        } // BCM2711: bus (VC) 0x7E00_0000 -> ARM phys 0xFE00_0000
        let arm = vc - 0x7E00_0000 + 0xFE00_0000;
        Some(AliasEntry {
            vc_addr: vc,
            arm_phys: arm,
            size: sz,
        })
    }
}
