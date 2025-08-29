use crate::uart_println;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FdtHeaderRaw {
    pub magic: u32,
    pub totalsize: u32,
    pub off_dt_struct: u32,
    pub off_dt_strings: u32,
    pub off_mem_rsvmap: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: u32,
    pub size_dt_struct: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct FdtHeader {
    pub magic: u32,
    pub totalsize: u32,
    pub off_dt_struct: u32,
    pub off_dt_strings: u32,
    pub off_mem_rsvmap: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: u32,
    pub size_dt_struct: u32,
}

impl FdtHeader {
    pub unsafe fn from_ptr(ptr: *const u8) -> Self {
        let raw = &*(ptr as *const FdtHeaderRaw);
        Self {
            magic: u32::from_be(raw.magic),
            totalsize: u32::from_be(raw.totalsize),
            off_dt_struct: u32::from_be(raw.off_dt_struct),
            off_dt_strings: u32::from_be(raw.off_dt_strings),
            off_mem_rsvmap: u32::from_be(raw.off_mem_rsvmap),
            version: u32::from_be(raw.version),
            last_comp_version: u32::from_be(raw.last_comp_version),
            boot_cpuid_phys: u32::from_be(raw.boot_cpuid_phys),
            size_dt_strings: u32::from_be(raw.size_dt_strings),
            size_dt_struct: u32::from_be(raw.size_dt_struct),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemRegion {
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ReservedRegion<'a> {
    pub base: u64,
    pub size: u64,
    pub name: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct PeripheralRegion {
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Peripheral<'a> {
    pub compatible: &'a str,
    pub name: &'a str,
    pub regions: [PeripheralRegion; Fdt::MAX_REGIONS_PERIPH],
    pub regions_found: usize,
}

pub struct Fdt<'a> {
    pub header: FdtHeader,
    pub struct_ptr: *const u8,
    pub strings_ptr: *const u8,
    pub regions: [MemRegion; Fdt::MAX_REGIONS],
    pub regions_found: usize,
    pub reserved: [ReservedRegion<'a>; Fdt::MAX_REGIONS],
    pub reserved_found: usize,
    pub peripherals: [Peripheral<'a>; Fdt::MAX_PERIPHERALS],
    pub peripherals_found: usize,
    pub address_cells: usize,
    pub size_cells: usize,
    pub dtb_addr: usize,
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<'a> Fdt<'a> {
    pub const FDT_MAGIC: u32 = 0xd00dfeed;
    pub const FDT_BEGIN_NODE: u32 = 0x1;
    pub const FDT_END_NODE: u32 = 0x2;
    pub const FDT_PROP: u32 = 0x3;
    pub const FDT_NOP: u32 = 0x4;
    pub const FDT_END: u32 = 0x9;
    pub const MAX_REGIONS: usize = 8;
    pub const MAX_PERIPHERALS: usize = 32;
    pub const MAX_REGIONS_PERIPH: usize = 8;

    /// Safe unaligned BE readers
    pub unsafe fn read_be_u32(ptr: *const u8) -> u32 {
        // read_unaligned works for any address/alignment
        let p = ptr as *const u32;
        u32::from_be(core::ptr::read_unaligned(p))
    }

    pub unsafe fn read_be_u64(ptr: *const u8) -> u64 {
        let p = ptr as *const u64;
        u64::from_be(core::ptr::read_unaligned(p))
    }

    // replace old read_u32_be/read_u64_be or keep as aliases:
    pub unsafe fn read_u32_be(ptr: *const u8) -> u32 { Self::read_be_u32(ptr) }
    pub unsafe fn read_u64_be(ptr: *const u8) -> u64 { Self::read_be_u64(ptr) }

    pub unsafe fn new(dtb_addr: usize) -> Option<Self> {
        // Use read_unaligned to read header fields instead of casting the whole struct pointer
        let magic = Self::read_be_u32(dtb_addr as *const u8);
        if magic != Self::FDT_MAGIC {
            uart_println!("DTB magic incorrect: {:#x}", magic);
            return None;
        }
        // read the rest of the header fields individually
        let totalsize = Self::read_be_u32((dtb_addr + 4) as *const u8);
        let off_dt_struct = Self::read_be_u32((dtb_addr + 8) as *const u8);
        let off_dt_strings = Self::read_be_u32((dtb_addr + 12) as *const u8);
        let off_mem_rsvmap = Self::read_be_u32((dtb_addr + 16) as *const u8);
        let version = Self::read_be_u32((dtb_addr + 20) as *const u8);
        let last_comp_version = Self::read_be_u32((dtb_addr + 24) as *const u8);
        let boot_cpuid_phys = Self::read_be_u32((dtb_addr + 28) as *const u8);
        let size_dt_strings = Self::read_be_u32((dtb_addr + 32) as *const u8);
        let size_dt_struct = Self::read_be_u32((dtb_addr + 36) as *const u8);

        let header = FdtHeader {
            magic,
            totalsize,
            off_dt_struct,
            off_dt_strings,
            off_mem_rsvmap,
            version,
            last_comp_version,
            boot_cpuid_phys,
            size_dt_strings,
            size_dt_struct,
        };

        let struct_ptr = (dtb_addr + header.off_dt_struct as usize) as *const u8;
        let strings_ptr = (dtb_addr + header.off_dt_strings as usize) as *const u8;

        Some(Self {
            header,
            struct_ptr,
            strings_ptr,
            regions: [MemRegion { base: 0, size: 0 }; Self::MAX_REGIONS],
            regions_found: 0,
            reserved: [ReservedRegion { base: 0, size: 0, name: "" }; Self::MAX_REGIONS],
            reserved_found: 0,
            peripherals: [Peripheral {
                compatible: "",
                name: "",
                regions: [PeripheralRegion { base: 0, size: 0 }; Fdt::MAX_REGIONS_PERIPH],
                regions_found: 0,
            }; Self::MAX_PERIPHERALS],
            peripherals_found: 0,
            // Keep defaults here, but parsing will use a stack to obey scoping
            address_cells: 2,
            size_cells: 1,
            dtb_addr,
            _phantom: core::marker::PhantomData,
        })
    }

    fn parse_cells(ptr: *const u8, cells: usize) -> u64 {
        let mut val = 0u64;
        for i in 0..cells {
            // read each 32-bit cell as big-endian via the safe helper
            let cell = unsafe { Self::read_be_u32(ptr.add(i * 4)) };
            val = (val << 32) | (cell as u64);
        }
        val
    }


    pub unsafe fn parse_all(&mut self, wanted_peripherals: &[&'a str]) {
        let mut ptr = self.struct_ptr;
        let mut depth: usize = 0;

        // stacks for #address-cells / #size-cells per depth
        const MAX_DEPTH: usize = 32;
        let mut addr_cells_stack = [self.address_cells; MAX_DEPTH];
        let mut size_cells_stack = [self.size_cells; MAX_DEPTH];
        let mut stack_top: usize = 0; // index of current node's values

        let mut in_reserved_mem = false;
        let mut reserved_mem_depth = 0usize;
        let mut in_memory_node = false;
        let mut in_peripheral_node = false;
        let mut node_name: &'a str = "";
        let mut found_compatible: Option<&'a str> = None;

        loop {
            let tag = Self::read_u32_be(ptr);
            ptr = ptr.add(4);
            match tag {
                Self::FDT_BEGIN_NODE => {
                    // push a new frame on the stack
                    depth += 1;
                    if stack_top + 1 >= MAX_DEPTH {
                        // too deep; bail out to avoid overflow
                        uart_println!("DTB node depth exceeded {}", MAX_DEPTH);
                        break;
                    }
                    // propagate parent's cell counts
                    addr_cells_stack[stack_top + 1] = addr_cells_stack[stack_top];
                    size_cells_stack[stack_top + 1] = size_cells_stack[stack_top];
                    stack_top += 1;

                    // read node name (NUL-terminated)
                    let mut name_ptr = ptr;
                    while *name_ptr != 0 {
                        name_ptr = name_ptr.add(1);
                    }
                    let name_len = name_ptr as usize - ptr as usize;
                    node_name = core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, name_len));

                    // flags based on node name
                    in_memory_node = node_name.starts_with("memory");
                    in_reserved_mem = node_name == "reserved-memory";
                    reserved_mem_depth = if in_reserved_mem { depth } else { reserved_mem_depth };
                    in_peripheral_node = false;
                    found_compatible = None;

                    ptr = name_ptr.add(1);
                    // align to 4
                    while (ptr as usize) % 4 != 0 {
                        ptr = ptr.add(1);
                    }
                }
                Self::FDT_PROP => {
                    let len = Self::read_u32_be(ptr) as usize;
                    let nameoff = Self::read_u32_be(ptr.add(4)) as usize;
                    ptr = ptr.add(8);
                    let prop_name_ptr = self.strings_ptr.add(nameoff);
                    let mut prop_name_end = prop_name_ptr;
                    while *prop_name_end != 0 {
                        prop_name_end = prop_name_end.add(1);
                    }
                    let prop_name_len = prop_name_end as usize - prop_name_ptr as usize;
                    let prop_name = core::str::from_utf8_unchecked(core::slice::from_raw_parts(prop_name_ptr, prop_name_len));

                    // local current cells for this node = top of stack
                    let cur_addr_cells = addr_cells_stack[stack_top];
                    let cur_size_cells = size_cells_stack[stack_top];

                    if prop_name == "#address-cells" && len == 4 {
                        // update current node's address-cells
                        let v = Self::read_be_u32(ptr) as usize;
                        addr_cells_stack[stack_top] = v;
                    }
                    if prop_name == "#size-cells" && len == 4 {
                        let v = Self::read_be_u32(ptr) as usize;
                        size_cells_stack[stack_top] = v;
                    }

                    if in_memory_node && prop_name == "reg" {
                        let tuple_size = (cur_addr_cells + cur_size_cells) * 4;
                        let num_regs = len / tuple_size;
                        for i in 0..num_regs {
                            let tuple_ptr = ptr.add(i * tuple_size);
                            let base = Self::parse_cells(tuple_ptr, cur_addr_cells);
                            let size = Self::parse_cells(tuple_ptr.add(cur_addr_cells * 4), cur_size_cells);
                            uart_println!("Parsed RAM region: base={:#x}, size={:#x}", base, size);
                            if self.regions_found < Self::MAX_REGIONS {
                                self.regions[self.regions_found] = MemRegion { base, size };
                                self.regions_found += 1;
                            }
                        }
                    }

                    if in_reserved_mem && depth == reserved_mem_depth + 1 && prop_name == "reg" {
                        let tuple_size = (cur_addr_cells + cur_size_cells) * 4;
                        let num_regs = len / tuple_size;
                        for i in 0..num_regs {
                            if self.reserved_found < Fdt::MAX_REGIONS {
                                let tuple_ptr = ptr.add(i * tuple_size);
                                let base = Self::parse_cells(tuple_ptr, cur_addr_cells);
                                let size = Self::parse_cells(tuple_ptr.add(cur_addr_cells * 4), cur_size_cells);
                                self.reserved[self.reserved_found] = ReservedRegion { base, size, name: node_name };
                                self.reserved_found += 1;
                            }
                        }
                    }

                    if prop_name == "compatible" {
                        let val = core::slice::from_raw_parts(ptr, len);
                        let mut start = 0usize;
                        while start < val.len() {
                            let mut end = start;
                            while end < val.len() && val[end] != 0 {
                                end += 1;
                            }
                            if end > start {
                                let compat_str = core::str::from_utf8_unchecked(&val[start..end]);
                                for &wanted in wanted_peripherals {
                                    if compat_str == wanted {
                                        in_peripheral_node = true;
                                        found_compatible = Some(compat_str);
                                    }
                                }
                            }
                            start = end + 1;
                        }
                    }

                    // Parse reg for peripherals using the current cell counts
                    if in_peripheral_node && prop_name == "reg" && found_compatible.is_some() {
                        let tuple_size = (cur_addr_cells + cur_size_cells) * 4;
                        let num_regs = len / tuple_size;
                        let mut periph = Peripheral {
                            compatible: found_compatible.unwrap(),
                            name: node_name,
                            regions: [PeripheralRegion { base: 0, size: 0 }; Fdt::MAX_REGIONS_PERIPH],
                            regions_found: 0,
                        };
                        for i in 0..num_regs {
                            let tuple_ptr = ptr.add(i * tuple_size);
                            let base = Self::parse_cells(tuple_ptr, cur_addr_cells);
                            let size = Self::parse_cells(tuple_ptr.add(cur_addr_cells * 4), cur_size_cells);
                            if periph.regions_found < Fdt::MAX_REGIONS_PERIPH {
                                periph.regions[periph.regions_found] = PeripheralRegion { base, size };
                                periph.regions_found += 1;
                            }
                        }
                        if self.peripherals_found < Fdt::MAX_PERIPHERALS {
                            self.peripherals[self.peripherals_found] = periph;
                            self.peripherals_found += 1;
                        }
                    }

                    // advance past the property payload (rounded up to 4)
                    ptr = ptr.add(((len + 3) & !3) as usize);
                }
                Self::FDT_END_NODE => {
                    // pop stack
                    in_memory_node = false;
                    in_peripheral_node = false;
                    if in_reserved_mem && depth == reserved_mem_depth {
                        in_reserved_mem = false;
                        reserved_mem_depth = 0;
                    }
                    if stack_top > 0 { stack_top -= 1; }
                    depth -= 1;
                }
                Self::FDT_END => break,
                _ => {
                    // NOP or unknown tag: ignore
                }
            }
        }
    }

    pub fn print_all(&self) {
        uart_println!("RAM regions:");
        let mut total_ram: u64 = 0;
        for i in 0..self.regions_found {
            let r = self.regions[i];
            total_ram += r.size;
            uart_println!("Region {}: base={:#x} size={:#x}", i, r.base, r.size);
        }
        uart_println!("Total RAM: {} MB", total_ram / (1024 * 1024));

        uart_println!("Reserved memory regions:");
        for i in 0..self.reserved_found {
            let r = self.reserved[i];
            uart_println!("Region {}: base={:#x} size={:#x} name={}", i, r.base, r.size, r.name);
        }

        uart_println!("Peripherals:");
        for i in 0..self.peripherals_found {
            let p = self.peripherals[i];
            uart_println!(
                "Peripheral {}: compatible={} name={}",
                i, p.compatible, p.name
            );
            for j in 0..p.regions_found {
                let r = p.regions[j];
                uart_println!(
                    "  Region {}: base={:#x} size={:#x}",
                    j, r.base, r.size
                );
            }
        }
    }
}
