//! Single-pass, allocation-free Flattened Device Tree boot parser.
//!
//! The structure block is traversed exactly once. During that traversal the
//! parser:
//!
//! - caches aliases as already translated `AliasEntry` values;
//! - records RAM from `device_type = "memory"` nodes;
//! - records the FDT reservation map;
//! - records fixed `/reserved-memory` children;
//! - preserves size-only `/reserved-memory` allocation requests;
//! - builds a non-overlapping list of usable physical RAM.
//!
//! The parser is intentionally focused on early boot. It does not build an
//! in-memory device-tree hierarchy and does not require `alloc`.
//!
//! `AliasEntry::vc_addr` is the address from the target node's first `reg`
//! tuple. `AliasEntry::arm_phys` is that address translated through every
//! ancestor bus `ranges` property up to the root.
//!
//! The default capacities are deliberately generous for the BCM2711 CM4 DTB.
//! Increase them if a future platform returns `Error::CapacityExceeded`.

use core::{fmt, slice, str};

const FDT_MAGIC: u32 = 0xd00d_feed;

const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_NOP: u32 = 0x0000_0004;
const FDT_END: u32 = 0x0000_0009;

const HEADER_SIZE: usize = 40;

pub const MAX_DEPTH: usize = 32;
pub const MAX_PATH_LEN: usize = 256;
pub const MAX_ALIASES: usize = 64;
pub const MAX_ALIAS_NAME_LEN: usize = 31;
pub const MAX_INDEXED_NODES: usize = 128;
pub const MAX_RAM_REGIONS: usize = 16;
pub const MAX_RESERVED_REGIONS: usize = 64;
pub const MAX_DYNAMIC_RESERVATIONS: usize = 16;
pub const MAX_USABLE_REGIONS: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NullPointer,
    HeaderTooSmall,
    BadMagic(u32),
    TotalSizeTooSmall,
    Truncated,
    InvalidOffset,
    InvalidStructure,
    InvalidStringOffset,
    InvalidUtf8,
    InvalidProperty,
    InvalidNodeNesting,
    NodeDepthExceeded,
    PathTooLong,
    AliasNameTooLong,
    UnsupportedAddressCells(u32),
    UnsupportedSizeCells(u32),
    InvalidRegLength,
    InvalidRangesLength,
    AddressOverflow,
    UntranslatableAddress(u64),
    CapacityExceeded(Capacity),
    MissingRootNode,
    MissingEndToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capacity {
    Aliases,
    IndexedNodes,
    RamRegions,
    ReservedRegions,
    DynamicReservations,
    UsableRegions,
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AliasEntry {
    /// Address encoded by the target node's first `reg` tuple.
    pub vc_addr: u64,

    /// Address after applying all ancestor bus `ranges` translations.
    pub arm_phys: u64,

    pub size: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SmallName {
    len: u8,
    bytes: [u8; MAX_ALIAS_NAME_LEN],
}

impl SmallName {
    pub const fn empty() -> Self {
        Self {
            len: 0,
            bytes: [0; MAX_ALIAS_NAME_LEN],
        }
    }

    pub fn new(value: &str) -> Result<Self> {
        let source = value.as_bytes();

        if source.len() > MAX_ALIAS_NAME_LEN {
            return Err(Error::AliasNameTooLong);
        }

        let mut name = Self::empty();
        name.bytes[..source.len()].copy_from_slice(source);
        name.len = source.len() as u8;
        Ok(name)
    }

    #[inline(always)]
    pub fn as_str(&self) -> &str {
        // All names originate from validated UTF-8 FDT strings.
        unsafe { str::from_utf8_unchecked(&self.bytes[..self.len as usize]) }
    }

    #[inline(always)]
    pub fn equals(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl Default for SmallName {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for SmallName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SmallName").field(&self.as_str()).finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AliasRecord {
    pub name: SmallName,
    pub entry: Option<AliasEntry>,
}

pub struct AliasTable {
    records: [AliasRecord; MAX_ALIASES],
    len: usize,
}

impl AliasTable {
    pub fn new() -> Self {
        Self {
            records: [AliasRecord::default(); MAX_ALIASES],
            len: 0,
        }
    }

    /// Create a table containing only the aliases requested by the kernel.
    ///
    /// Names are copied into the fixed-capacity table, so the returned
    /// `AliasTable` does not borrow `requested`.
    pub fn with_requested(requested: &[&str]) -> Result<Self> {
        let mut table = Self::new();

        for name in requested {
            table.insert_or_get(name)?;
        }

        Ok(table)
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn records(&self) -> &[AliasRecord] {
        &self.records[..self.len]
    }

    /// Cheap cached lookup. This never reads or traverses the DTB.
    #[inline]
    pub fn get(&self, name: &str) -> Option<AliasEntry> {
        self.records()
            .iter()
            .find(|record| record.name.equals(name))
            .and_then(|record| record.entry)
    }

    pub fn unresolved(&self) -> impl Iterator<Item = &AliasRecord> {
        self.records().iter().filter(|record| record.entry.is_none())
    }

    fn find_index(&self, name: &str) -> Option<usize> {
        self.records()
            .iter()
            .position(|record| record.name.equals(name))
    }

    fn insert_or_get(&mut self, name: &str) -> Result<usize> {
        if let Some(index) = self.find_index(name) {
            return Ok(index);
        }

        if self.len == MAX_ALIASES {
            return Err(Error::CapacityExceeded(Capacity::Aliases));
        }

        let index = self.len;
        self.records[index] = AliasRecord {
            name: SmallName::new(name)?,
            entry: None,
        };
        self.len += 1;
        Ok(index)
    }

    fn resolve(&mut self, index: usize, entry: AliasEntry) {
        self.records[index].entry = Some(entry);
    }
}

impl Default for AliasTable {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AliasTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AliasTable")
            .field("records", &self.records())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhysRange {
    pub start: u64,
    pub size: u64,
}

impl PhysRange {
    pub const fn new(start: u64, size: u64) -> Self {
        Self { start, size }
    }

    #[inline(always)]
    pub fn end(self) -> u64 {
        self.start.saturating_add(self.size)
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.size == 0
    }

    #[inline(always)]
    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MemoryKind {
    #[default]
    Reserved,
    NoMap,
    Dma,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryReservation {
    pub range: PhysRange,
    pub kind: MemoryKind,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DynamicReservation {
    pub name: SmallName,
    pub size: u64,
    pub alignment: u64,
    pub alloc_range: Option<PhysRange>,
    pub kind: MemoryKind,
    pub reusable: bool,
}

#[derive(Clone, Copy)]
struct FixedVec<T: Copy + Default, const N: usize> {
    items: [T; N],
    len: usize,
}

impl<T: Copy + Default, const N: usize> FixedVec<T, N> {
    fn new() -> Self {
        Self {
            items: [T::default(); N],
            len: 0,
        }
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    fn as_slice(&self) -> &[T] {
        &self.items[..self.len]
    }

    #[inline(always)]
    fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.len]
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, value: T, capacity: Capacity) -> Result<()> {
        if self.len == N {
            return Err(Error::CapacityExceeded(capacity));
        }

        self.items[self.len] = value;
        self.len += 1;
        Ok(())
    }
}

impl<T: Copy + Default, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MemoryLayout {
    ram: FixedVec<PhysRange, MAX_RAM_REGIONS>,
    reservations: FixedVec<MemoryReservation, MAX_RESERVED_REGIONS>,
    dynamic: FixedVec<DynamicReservation, MAX_DYNAMIC_RESERVATIONS>,
    usable: FixedVec<PhysRange, MAX_USABLE_REGIONS>,
}

impl MemoryLayout {
    pub fn new() -> Self {
        Self {
            ram: FixedVec::new(),
            reservations: FixedVec::new(),
            dynamic: FixedVec::new(),
            usable: FixedVec::new(),
        }
    }

    #[inline(always)]
    pub fn ram_regions(&self) -> &[PhysRange] {
        self.ram.as_slice()
    }

    #[inline(always)]
    pub fn reservations(&self) -> &[MemoryReservation] {
        self.reservations.as_slice()
    }

    #[inline(always)]
    pub fn dynamic_reservations(&self) -> &[DynamicReservation] {
        self.dynamic.as_slice()
    }

    /// RAM after every fixed reservation has been subtracted.
    #[inline(always)]
    pub fn usable_regions(&self) -> &[PhysRange] {
        self.usable.as_slice()
    }

    pub fn add_ram(&mut self, start: u64, size: u64) -> Result<()> {
        validate_range(start, size)?;

        if size == 0 {
            return Ok(());
        }

        self.ram.push(
            PhysRange::new(start, size),
            Capacity::RamRegions,
        )
    }

    pub fn reserve(&mut self, start: u64, size: u64, kind: MemoryKind) -> Result<()> {
        validate_range(start, size)?;

        if size == 0 {
            return Ok(());
        }

        self.reservations.push(
            MemoryReservation {
                range: PhysRange::new(start, size),
                kind,
            },
            Capacity::ReservedRegions,
        )
    }

    pub fn add_dynamic_reservation(&mut self, request: DynamicReservation) -> Result<()> {
        if request.size == 0 {
            return Ok(());
        }

        self.dynamic
            .push(request, Capacity::DynamicReservations)
    }

    /// Add a kernel/runtime reservation and regenerate the allocator-safe RAM
    /// list. This is intended for the kernel image, stacks, DTB, framebuffer,
    /// temporary heap, and any later fixed allocation.
    pub fn reserve_runtime(
        &mut self,
        start: u64,
        size: u64,
        kind: MemoryKind,
    ) -> Result<()> {
        self.reserve(start, size, kind)?;
        self.rebuild_usable()
    }

    /// Sort RAM and subtract all fixed reservations.
    ///
    /// `usable_regions()` is guaranteed not to overlap anything returned by
    /// `reservations()`.
    pub fn rebuild_usable(&mut self) -> Result<()> {
        sort_ranges(self.ram.as_mut_slice());
        coalesce_ranges(&mut self.ram);

        let mut exclusions: FixedVec<PhysRange, MAX_RESERVED_REGIONS> = FixedVec::new();

        for reservation in self.reservations() {
            exclusions.push(
                reservation.range,
                Capacity::ReservedRegions,
            )?;
        }

        sort_ranges(exclusions.as_mut_slice());
        coalesce_ranges(&mut exclusions);

        let mut current: FixedVec<PhysRange, MAX_USABLE_REGIONS> = FixedVec::new();

        for ram in self.ram_regions() {
            current.push(*ram, Capacity::UsableRegions)?;
        }

        for excluded in exclusions.as_slice() {
            let mut next: FixedVec<PhysRange, MAX_USABLE_REGIONS> = FixedVec::new();

            for region in current.as_slice() {
                subtract_range(
                    *region,
                    *excluded,
                    &mut next,
                )?;
            }

            current = next;
        }

        sort_ranges(current.as_mut_slice());
        coalesce_ranges(&mut current);
        self.usable = current;
        Ok(())
    }
}

impl Default for MemoryLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MemoryLayout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryLayout")
            .field("ram", &self.ram_regions())
            .field("reservations", &self.reservations())
            .field("dynamic_reservations", &self.dynamic_reservations())
            .field("usable", &self.usable_regions())
            .finish()
    }
}

pub struct BootInfo {
    pub aliases: AliasTable,
    pub memory: MemoryLayout,
}

impl BootInfo {
    /// Return a previously requested and resolved alias.
    ///
    /// This is a cached fixed-array lookup and never traverses the DTB.
    #[inline(always)]
    pub fn get(&self, name: &str) -> Option<AliasEntry> {
        self.aliases.get(name)
    }

    /// Parse an FDT from a complete byte slice, caching only the requested
    /// aliases while still collecting the complete boot memory layout.
    pub fn from_bytes(bytes: &[u8], requested_aliases: &[&str]) -> Result<Self> {
        let dtb = Dtb::from_bytes(bytes)?;
        Parser::new(dtb, requested_aliases)?.parse()
    }

    /// Parse an FDT supplied by firmware in memory, caching only the requested
    /// aliases while still collecting the complete boot memory layout.
    ///
    /// # Safety
    ///
    /// `pointer` must point to a readable FDT header and to at least the
    /// `totalsize` bytes declared by that header. The memory must remain
    /// readable for the duration of this call.
    pub unsafe fn from_ptr(
        pointer: *const u8,
        requested_aliases: &[&str],
    ) -> Result<Self> {
        if pointer.is_null() {
            return Err(Error::NullPointer);
        }

        let header = unsafe { slice::from_raw_parts(pointer, HEADER_SIZE) };
        let total_size = read_be_u32(header, 4)? as usize;

        if total_size < HEADER_SIZE {
            return Err(Error::TotalSizeTooSmall);
        }

        let bytes = unsafe { slice::from_raw_parts(pointer, total_size) };
        Self::from_bytes(bytes, requested_aliases)
    }
}

#[derive(Clone, Copy)]
struct Header {
    total_size: usize,
    structure_offset: usize,
    strings_offset: usize,
    reservation_offset: usize,
    strings_size: usize,
    structure_size: usize,
}

#[derive(Clone, Copy)]
struct Dtb<'a> {
    bytes: &'a [u8],
    header: Header,
    structure: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Dtb<'a> {
    fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::HeaderTooSmall);
        }

        let magic = read_be_u32(bytes, 0)?;

        if magic != FDT_MAGIC {
            return Err(Error::BadMagic(magic));
        }

        let total_size = read_be_u32(bytes, 4)? as usize;
        let structure_offset = read_be_u32(bytes, 8)? as usize;
        let strings_offset = read_be_u32(bytes, 12)? as usize;
        let reservation_offset = read_be_u32(bytes, 16)? as usize;
        let strings_size = read_be_u32(bytes, 32)? as usize;
        let structure_size = read_be_u32(bytes, 36)? as usize;

        if total_size < HEADER_SIZE || total_size > bytes.len() {
            return Err(Error::TotalSizeTooSmall);
        }

        let structure_end = structure_offset
            .checked_add(structure_size)
            .ok_or(Error::InvalidOffset)?;
        let strings_end = strings_offset
            .checked_add(strings_size)
            .ok_or(Error::InvalidOffset)?;

        if structure_offset < HEADER_SIZE
            || strings_offset < HEADER_SIZE
            || reservation_offset < HEADER_SIZE
            || structure_end > total_size
            || strings_end > total_size
            || reservation_offset >= total_size
        {
            return Err(Error::InvalidOffset);
        }

        let header = Header {
            total_size,
            structure_offset,
            strings_offset,
            reservation_offset,
            strings_size,
            structure_size,
        };

        Ok(Self {
            bytes: &bytes[..total_size],
            header,
            structure: &bytes[structure_offset..structure_end],
            strings: &bytes[strings_offset..strings_end],
        })
    }

    fn property_name(&self, offset: usize) -> Result<&'a str> {
        if offset >= self.strings.len() {
            return Err(Error::InvalidStringOffset);
        }

        read_cstr(&self.strings[offset..])
    }

    fn reservation_limit(&self) -> usize {
        self.header
            .structure_offset
            .min(self.header.strings_offset)
            .min(self.header.total_size)
    }
}

#[derive(Clone, Copy, Default)]
struct PendingAlias<'a> {
    target: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PathFingerprint {
    first: u64,
    second: u64,
    len: u16,
}

impl PathFingerprint {
    fn from_bytes(bytes: &[u8]) -> Self {
        const OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;
        const OFFSET_B: u64 = 0x8422_2325_cbf2_9ce4;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut first = OFFSET_A;
        let mut second = OFFSET_B;

        for byte in bytes {
            first ^= u64::from(*byte);
            first = first.wrapping_mul(PRIME);

            second ^= u64::from(*byte).rotate_left(1);
            second = second.wrapping_mul(PRIME.rotate_left(7) | 1);
        }

        Self {
            first,
            second,
            len: bytes.len().min(u16::MAX as usize) as u16,
        }
    }

    #[inline(always)]
    fn matches(self, other: Self) -> bool {
        self.first == other.first
            && self.second == other.second
            && self.len == other.len
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct IndexedNode {
    fingerprint: PathFingerprint,
    entry: AliasEntry,
}

#[derive(Clone, Copy)]
struct NodeFrame<'a> {
    parent_path_len: usize,
    name: &'a str,

    /// Encoding used by this node's children.
    child_address_cells: u32,
    child_size_cells: u32,

    reg: Option<&'a [u8]>,
    ranges: Option<&'a [u8]>,
    ranges_present: bool,

    device_type: Option<&'a str>,
    compatible: Option<&'a [u8]>,
    status: Option<&'a str>,

    size: Option<&'a [u8]>,
    alignment: Option<&'a [u8]>,
    alloc_ranges: Option<&'a [u8]>,

    no_map: bool,
    reusable: bool,
    is_reserved_memory_root: bool,
    parent_is_reserved_memory_root: bool,
    saw_child: bool,
}

impl<'a> NodeFrame<'a> {
    const fn empty() -> Self {
        Self {
            parent_path_len: 0,
            name: "",
            child_address_cells: 2,
            child_size_cells: 1,
            reg: None,
            ranges: None,
            ranges_present: false,
            device_type: None,
            compatible: None,
            status: None,
            size: None,
            alignment: None,
            alloc_ranges: None,
            no_map: false,
            reusable: false,
            is_reserved_memory_root: false,
            parent_is_reserved_memory_root: false,
            saw_child: false,
        }
    }

    #[inline(always)]
    fn enabled(self) -> bool {
        match self.status {
            None | Some("okay") | Some("ok") => true,
            Some("disabled") | Some("reserved") | Some("fail") | Some("failed") => false,
            Some(_) => true,
        }
    }

    #[inline(always)]
    fn is_memory(self) -> bool {
        self.device_type == Some("memory") || self.name.starts_with("memory@")
    }

    #[inline(always)]
    fn is_dma_pool(self) -> bool {
        self.compatible
            .is_some_and(|value| string_list_contains(value, "shared-dma-pool"))
    }
}

struct Parser<'a> {
    dtb: Dtb<'a>,

    aliases: AliasTable,
    pending_aliases: [PendingAlias<'a>; MAX_ALIASES],

    indexed_nodes: [IndexedNode; MAX_INDEXED_NODES],
    indexed_node_len: usize,

    memory: MemoryLayout,

    stack: [NodeFrame<'a>; MAX_DEPTH],
    depth: usize,

    path: [u8; MAX_PATH_LEN],
    path_len: usize,

    saw_root: bool,
    saw_end: bool,
}

impl<'a> Parser<'a> {
    fn new(dtb: Dtb<'a>, requested_aliases: &[&str]) -> Result<Self> {
        let aliases = AliasTable::with_requested(requested_aliases)?;

        let mut parser = Self {
            dtb,
            aliases,
            pending_aliases: [PendingAlias::default(); MAX_ALIASES],
            indexed_nodes: [IndexedNode::default(); MAX_INDEXED_NODES],
            indexed_node_len: 0,
            memory: MemoryLayout::new(),
            stack: [NodeFrame::empty(); MAX_DEPTH],
            depth: 0,
            path: [0; MAX_PATH_LEN],
            path_len: 0,
            saw_root: false,
            saw_end: false,
        };

        parser.parse_reservation_map()?;
        Ok(parser)
    }

    fn parse(mut self) -> Result<BootInfo> {
        let mut cursor = 0usize;

        while cursor < self.dtb.structure.len() {
            let token = take_be_u32(self.dtb.structure, &mut cursor)?;

            match token {
                FDT_BEGIN_NODE => {
                    let name = take_cstr(self.dtb.structure, &mut cursor)?;
                    cursor = align_up(cursor, 4)?;
                    self.begin_node(name)?;
                }

                FDT_END_NODE => {
                    self.end_node()?;
                }

                FDT_PROP => {
                    let length = take_be_u32(self.dtb.structure, &mut cursor)? as usize;
                    let name_offset = take_be_u32(self.dtb.structure, &mut cursor)? as usize;

                    let end = cursor.checked_add(length).ok_or(Error::Truncated)?;

                    if end > self.dtb.structure.len() {
                        return Err(Error::Truncated);
                    }

                    let value = &self.dtb.structure[cursor..end];
                    cursor = align_up(end, 4)?;

                    let name = self.dtb.property_name(name_offset)?;
                    self.property(name, value)?;
                }

                FDT_NOP => {}

                FDT_END => {
                    if self.depth != 0 {
                        return Err(Error::InvalidNodeNesting);
                    }

                    self.saw_end = true;
                    break;
                }

                _ => return Err(Error::InvalidStructure),
            }
        }

        if !self.saw_root {
            return Err(Error::MissingRootNode);
        }

        if !self.saw_end {
            return Err(Error::MissingEndToken);
        }

        self.memory.rebuild_usable()?;

        Ok(BootInfo {
            aliases: self.aliases,
            memory: self.memory,
        })
    }

    fn parse_reservation_map(&mut self) -> Result<()> {
        let mut cursor = self.dtb.header.reservation_offset;
        let limit = self.dtb.reservation_limit();

        loop {
            let pair_end = cursor.checked_add(16).ok_or(Error::Truncated)?;

            if pair_end > limit || pair_end > self.dtb.bytes.len() {
                return Err(Error::Truncated);
            }

            let address = read_be_u64(self.dtb.bytes, cursor)?;
            let size = read_be_u64(self.dtb.bytes, cursor + 8)?;
            cursor = pair_end;

            if address == 0 && size == 0 {
                return Ok(());
            }

            self.memory
                .reserve(address, size, MemoryKind::Reserved)?;
        }
    }

    fn begin_node(&mut self, name: &'a str) -> Result<()> {
        if self.depth == MAX_DEPTH {
            return Err(Error::NodeDepthExceeded);
        }

        if self.depth > 0 {
            self.stack[self.depth - 1].saw_child = true;
        }

        let parent_path_len = self.path_len;

        if self.depth == 0 {
            if !name.is_empty() {
                return Err(Error::InvalidStructure);
            }

            self.path[0] = b'/';
            self.path_len = 1;
            self.saw_root = true;
        } else {
            if self.path_len > 1 {
                self.push_path_bytes(b"/")?;
            }

            self.push_path_bytes(name.as_bytes())?;
        }

        let parent_is_reserved_memory_root = self.depth > 0
            && self.stack[self.depth - 1].is_reserved_memory_root;

        let mut frame = NodeFrame::empty();
        frame.parent_path_len = parent_path_len;
        frame.name = name;
        frame.parent_is_reserved_memory_root = parent_is_reserved_memory_root;
        frame.is_reserved_memory_root = self.current_path() == b"/reserved-memory";

        self.stack[self.depth] = frame;
        self.depth += 1;
        Ok(())
    }

    fn property(&mut self, name: &'a str, value: &'a [u8]) -> Result<()> {
        if self.depth == 0 {
            return Err(Error::InvalidNodeNesting);
        }

        if self.current_path() == b"/aliases" {
            self.alias_property(name, value)?;
        }

        let frame = &mut self.stack[self.depth - 1];

        // FDT properties are required to precede child nodes. Refuse a tree
        // that would make a true streaming parse ambiguous.
        if frame.saw_child {
            return Err(Error::InvalidProperty);
        }

        match name {
            "#address-cells" => {
                // Preserve arbitrary cell counts. The boot extractor only
                // decodes tuples using up to 64-bit (two-cell) addresses, but
                // unrelated buses such as PCI legitimately use three cells.
                frame.child_address_cells = parse_single_u32(value)?;
            }

            "#size-cells" => {
                frame.child_size_cells = parse_single_u32(value)?;
            }

            "reg" => frame.reg = Some(value),

            "ranges" => {
                frame.ranges_present = true;
                frame.ranges = Some(value);
            }

            "device_type" => frame.device_type = Some(read_cstr(value)?),
            "compatible" => frame.compatible = Some(value),
            "status" => frame.status = Some(read_cstr(value)?),
            "size" => frame.size = Some(value),
            "alignment" => frame.alignment = Some(value),
            "alloc-ranges" => frame.alloc_ranges = Some(value),
            "no-map" => frame.no_map = true,
            "reusable" => frame.reusable = true,
            _ => {}
        }

        Ok(())
    }

    fn end_node(&mut self) -> Result<()> {
        if self.depth == 0 {
            return Err(Error::InvalidNodeNesting);
        }

        let index = self.depth - 1;
        let frame = self.stack[index];

        self.finalize_node(index, frame)?;

        self.depth -= 1;
        self.path_len = frame.parent_path_len;
        Ok(())
    }

    fn finalize_node(&mut self, index: usize, frame: NodeFrame<'a>) -> Result<()> {
        if !frame.enabled() {
            return Ok(());
        }

        let first_reg = match frame.reg {
            Some(reg) => match self.decode_first_reg(index, reg) {
                Ok(value) => value,
                // Many non-MMIO nodes (CPU IDs, child-local registers, and
                // similar metadata) intentionally have no root translation.
                // They are irrelevant to the MMIO alias cache, so do not make
                // the whole boot parse fail because they cannot be indexed.
                Err(
                    Error::UntranslatableAddress(_)
                    | Error::UnsupportedAddressCells(_)
                    | Error::UnsupportedSizeCells(_)
                    | Error::InvalidRegLength,
                ) => None,
                Err(error) => return Err(error),
            },
            None => None,
        };

        if let Some((bus_address, physical_address, size)) = first_reg {
            if size != 0 {
                let entry = AliasEntry {
                    vc_addr: bus_address,
                    arm_phys: physical_address,
                    size,
                };

                self.index_current_node(entry)?;
                self.resolve_aliases_for_current_path(entry);
            }
        }

        if frame.is_memory() {
            if let Some(reg) = frame.reg {
                self.decode_all_reg(index, reg, |parser, physical, size| {
                    parser.memory.add_ram(physical, size)
                })?;
            }
        }

        if frame.parent_is_reserved_memory_root {
            let kind = if frame.no_map {
                MemoryKind::NoMap
            } else if frame.is_dma_pool() {
                MemoryKind::Dma
            } else {
                MemoryKind::Reserved
            };

            if let Some(reg) = frame.reg {
                self.decode_all_reg(index, reg, |parser, physical, size| {
                    parser.memory.reserve(physical, size, kind)
                })?;
            } else if let Some(size_data) = frame.size {
                let parent = self.parent_frame(index)?;
                let size = decode_cells_exact(size_data, parent.child_size_cells)?;

                if size != 0 {
                    let alignment = match frame.alignment {
                        Some(data) => decode_cells_exact(data, parent.child_size_cells)?,
                        None => 1,
                    };

                    let alloc_range = match frame.alloc_ranges {
                        Some(data) => self.decode_first_address_size(
                            index,
                            data,
                            parent.child_address_cells,
                            parent.child_size_cells,
                        )?,
                        None => None,
                    }
                    .map(|(_, physical, size)| PhysRange::new(physical, size));

                    self.memory.add_dynamic_reservation(DynamicReservation {
                        name: SmallName::new(frame.name)?,
                        size,
                        alignment: alignment.max(1),
                        alloc_range,
                        kind,
                        reusable: frame.reusable,
                    })?;
                }
            }
        }

        Ok(())
    }

    fn alias_property(&mut self, name: &'a str, value: &'a [u8]) -> Result<()> {
        // `/aliases` can also contain binary metadata such as `phandle`.
        // Only an absolute NUL-terminated path is an alias property.
        if value.first().copied() != Some(b'/') {
            return Ok(());
        }

        let target = read_cstr(value)?;

        // Ignore every alias that the caller did not explicitly request.
        let Some(alias_index) = self.aliases.find_index(name) else {
            return Ok(());
        };

        self.pending_aliases[alias_index].target = Some(target);

        let fingerprint = PathFingerprint::from_bytes(target.as_bytes());

        if let Some(indexed) = self.indexed_nodes[..self.indexed_node_len]
            .iter()
            .find(|node| node.fingerprint.matches(fingerprint))
        {
            self.aliases.resolve(alias_index, indexed.entry);
        }

        Ok(())
    }

    fn resolve_aliases_for_current_path(&mut self, entry: AliasEntry) {
        for index in 0..self.aliases.len() {
            if self.aliases.records[index].entry.is_some() {
                continue;
            }

            let matches = {
                let current = &self.path[..self.path_len];
                self.pending_aliases[index]
                    .target
                    .is_some_and(|target| target.as_bytes() == current)
            };

            if matches {
                self.aliases.resolve(index, entry);
            }
        }
    }

    fn index_current_node(&mut self, entry: AliasEntry) -> Result<()> {
        if self.indexed_node_len == MAX_INDEXED_NODES {
            return Err(Error::CapacityExceeded(Capacity::IndexedNodes));
        }

        self.indexed_nodes[self.indexed_node_len] = IndexedNode {
            fingerprint: PathFingerprint::from_bytes(self.current_path()),
            entry,
        };
        self.indexed_node_len += 1;
        Ok(())
    }

    fn decode_first_reg(
        &self,
        node_index: usize,
        data: &'a [u8],
    ) -> Result<Option<(u64, u64, u64)>> {
        let parent = self.parent_frame(node_index)?;
        self.decode_first_address_size(
            node_index,
            data,
            parent.child_address_cells,
            parent.child_size_cells,
        )
    }

    fn decode_first_address_size(
        &self,
        node_index: usize,
        data: &'a [u8],
        address_cells: u32,
        size_cells: u32,
    ) -> Result<Option<(u64, u64, u64)>> {
        ensure_address_cells(address_cells)?;
        ensure_size_cells(size_cells)?;

        let tuple_cells = address_cells
            .checked_add(size_cells)
            .ok_or(Error::InvalidRegLength)? as usize;
        let tuple_bytes = tuple_cells
            .checked_mul(4)
            .ok_or(Error::InvalidRegLength)?;

        if tuple_bytes == 0 || data.len() < tuple_bytes || data.len() % tuple_bytes != 0 {
            return Err(Error::InvalidRegLength);
        }

        let bus_address = decode_cells(&data[..address_cells as usize * 4], address_cells)?;
        let size_offset = address_cells as usize * 4;
        let size = decode_cells(
            &data[size_offset..size_offset + size_cells as usize * 4],
            size_cells,
        )?;
        if size == 0 {
            return Ok(Some((bus_address, bus_address, 0)));
        }

        let physical = self.translate_address(node_index, bus_address, size)?;

        Ok(Some((bus_address, physical, size)))
    }

    fn decode_all_reg<F>(
        &mut self,
        node_index: usize,
        data: &'a [u8],
        mut callback: F,
    ) -> Result<()>
    where
        F: FnMut(&mut Self, u64, u64) -> Result<()>,
    {
        let parent = self.parent_frame(node_index)?;
        let address_cells = parent.child_address_cells;
        let size_cells = parent.child_size_cells;

        ensure_address_cells(address_cells)?;
        ensure_size_cells(size_cells)?;

        let tuple_cells = address_cells
            .checked_add(size_cells)
            .ok_or(Error::InvalidRegLength)? as usize;
        let tuple_bytes = tuple_cells
            .checked_mul(4)
            .ok_or(Error::InvalidRegLength)?;

        if tuple_bytes == 0 || data.len() % tuple_bytes != 0 {
            return Err(Error::InvalidRegLength);
        }

        for tuple in data.chunks_exact(tuple_bytes) {
            let bus_address = decode_cells(
                &tuple[..address_cells as usize * 4],
                address_cells,
            )?;
            let size_offset = address_cells as usize * 4;
            let size = decode_cells(
                &tuple[size_offset..size_offset + size_cells as usize * 4],
                size_cells,
            )?;

            if size == 0 {
                continue;
            }

            let physical = self.translate_address(node_index, bus_address, size)?;
            callback(self, physical, size)?;
        }

        Ok(())
    }

    fn translate_address(
        &self,
        node_index: usize,
        mut address: u64,
        size: u64,
    ) -> Result<u64> {
        if node_index == 0 {
            return Ok(address);
        }

        let mut bus_index = node_index - 1;

        // The root has no parent address space, so stop before applying a
        // translation at stack index zero.
        while bus_index > 0 {
            let bus = self.stack[bus_index];
            let parent = self.stack[bus_index - 1];

            if bus.ranges_present {
                let ranges = bus.ranges.unwrap_or(&[]);

                if !ranges.is_empty() {
                    address = translate_one_bus(
                        ranges,
                        bus.child_address_cells,
                        parent.child_address_cells,
                        bus.child_size_cells,
                        address,
                        size,
                    )?;
                }
                // Empty `ranges` means identity.
            } else {
                // A missing `ranges` means there is no defined translation
                // between these address spaces.
                return Err(Error::UntranslatableAddress(address));
            }

            bus_index -= 1;
        }

        Ok(address)
    }

    fn parent_frame(&self, node_index: usize) -> Result<NodeFrame<'a>> {
        if node_index == 0 {
            return Err(Error::InvalidNodeNesting);
        }

        Ok(self.stack[node_index - 1])
    }

    #[inline(always)]
    fn current_path(&self) -> &[u8] {
        &self.path[..self.path_len]
    }

    fn push_path_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let end = self
            .path_len
            .checked_add(bytes.len())
            .ok_or(Error::PathTooLong)?;

        if end > MAX_PATH_LEN {
            return Err(Error::PathTooLong);
        }

        self.path[self.path_len..end].copy_from_slice(bytes);
        self.path_len = end;
        Ok(())
    }
}

fn translate_one_bus(
    ranges: &[u8],
    child_address_cells: u32,
    parent_address_cells: u32,
    size_cells: u32,
    address: u64,
    requested_size: u64,
) -> Result<u64> {
    ensure_address_cells(child_address_cells)?;
    ensure_address_cells(parent_address_cells)?;
    ensure_size_cells(size_cells)?;

    let tuple_cells = child_address_cells
        .checked_add(parent_address_cells)
        .and_then(|value| value.checked_add(size_cells))
        .ok_or(Error::InvalidRangesLength)? as usize;
    let tuple_bytes = tuple_cells
        .checked_mul(4)
        .ok_or(Error::InvalidRangesLength)?;

    if tuple_bytes == 0 || ranges.len() % tuple_bytes != 0 {
        return Err(Error::InvalidRangesLength);
    }

    for tuple in ranges.chunks_exact(tuple_bytes) {
        let child_bytes = child_address_cells as usize * 4;
        let parent_bytes = parent_address_cells as usize * 4;

        let child_base = decode_cells(
            &tuple[..child_bytes],
            child_address_cells,
        )?;
        let parent_base = decode_cells(
            &tuple[child_bytes..child_bytes + parent_bytes],
            parent_address_cells,
        )?;
        let range_size = decode_cells(
            &tuple[child_bytes + parent_bytes..],
            size_cells,
        )?;

        let requested_end = address
            .checked_add(requested_size)
            .ok_or(Error::AddressOverflow)?;
        let range_end = child_base
            .checked_add(range_size)
            .ok_or(Error::AddressOverflow)?;

        if address >= child_base && requested_end <= range_end {
            let offset = address - child_base;
            return parent_base
                .checked_add(offset)
                .ok_or(Error::AddressOverflow);
        }
    }

    Err(Error::UntranslatableAddress(address))
}

fn decode_cells_exact(data: &[u8], cells: u32) -> Result<u64> {
    let expected = cells as usize * 4;

    if data.len() != expected {
        return Err(Error::InvalidProperty);
    }

    decode_cells(data, cells)
}

fn decode_cells(data: &[u8], cells: u32) -> Result<u64> {
    match cells {
        0 => Ok(0),

        1 => {
            if data.len() < 4 {
                return Err(Error::Truncated);
            }

            Ok(u64::from(read_be_u32(data, 0)?))
        }

        2 => {
            if data.len() < 8 {
                return Err(Error::Truncated);
            }

            Ok((u64::from(read_be_u32(data, 0)?) << 32)
                | u64::from(read_be_u32(data, 4)?))
        }

        value => Err(Error::UnsupportedAddressCells(value)),
    }
}

fn ensure_address_cells(cells: u32) -> Result<()> {
    if cells <= 2 {
        Ok(())
    } else {
        Err(Error::UnsupportedAddressCells(cells))
    }
}

fn ensure_size_cells(cells: u32) -> Result<()> {
    if cells <= 2 {
        Ok(())
    } else {
        Err(Error::UnsupportedSizeCells(cells))
    }
}

fn parse_single_u32(data: &[u8]) -> Result<u32> {
    if data.len() != 4 {
        return Err(Error::InvalidProperty);
    }

    read_be_u32(data, 0)
}

fn validate_range(start: u64, size: u64) -> Result<()> {
    start
        .checked_add(size)
        .ok_or(Error::AddressOverflow)
        .map(|_| ())
}

fn subtract_range(
    source: PhysRange,
    excluded: PhysRange,
    output: &mut FixedVec<PhysRange, MAX_USABLE_REGIONS>,
) -> Result<()> {
    if !source.overlaps(excluded) {
        return output.push(source, Capacity::UsableRegions);
    }

    let source_end = source.end();
    let excluded_end = excluded.end();

    if source.start < excluded.start {
        output.push(
            PhysRange::new(
                source.start,
                excluded.start - source.start,
            ),
            Capacity::UsableRegions,
        )?;
    }

    if excluded_end < source_end {
        output.push(
            PhysRange::new(
                excluded_end,
                source_end - excluded_end,
            ),
            Capacity::UsableRegions,
        )?;
    }

    Ok(())
}

fn sort_ranges(ranges: &mut [PhysRange]) {
    for index in 1..ranges.len() {
        let value = ranges[index];
        let mut position = index;

        while position > 0
            && (
                ranges[position - 1].start > value.start
                    || (
                        ranges[position - 1].start == value.start
                            && ranges[position - 1].size > value.size
                    )
            )
        {
            ranges[position] = ranges[position - 1];
            position -= 1;
        }

        ranges[position] = value;
    }
}

fn coalesce_ranges<const N: usize>(ranges: &mut FixedVec<PhysRange, N>) {
    if ranges.len() < 2 {
        return;
    }

    let mut write = 0usize;

    for read in 1..ranges.len {
        let current = ranges.items[write];
        let next = ranges.items[read];

        if next.start <= current.end() {
            let end = current.end().max(next.end());
            ranges.items[write].size = end - current.start;
        } else {
            write += 1;
            ranges.items[write] = next;
        }
    }

    ranges.len = write + 1;
}

fn string_list_contains(value: &[u8], needle: &str) -> bool {
    value
        .split(|byte| *byte == 0)
        .any(|part| part == needle.as_bytes())
}

fn read_cstr(bytes: &[u8]) -> Result<&str> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(Error::Truncated)?;

    str::from_utf8(&bytes[..end]).map_err(|_| Error::InvalidUtf8)
}

fn take_cstr<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a str> {
    if *cursor >= bytes.len() {
        return Err(Error::Truncated);
    }

    let value = read_cstr(&bytes[*cursor..])?;
    *cursor = (*cursor)
        .checked_add(value.len() + 1)
        .ok_or(Error::Truncated)?;
    Ok(value)
}

fn align_up(value: usize, alignment: usize) -> Result<usize> {
    let mask = alignment.checked_sub(1).ok_or(Error::InvalidOffset)?;

    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or(Error::InvalidOffset)
}

fn take_be_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let value = read_be_u32(bytes, *cursor)?;
    *cursor = (*cursor).checked_add(4).ok_or(Error::Truncated)?;
    Ok(value)
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or(Error::Truncated)?;

    if end > bytes.len() {
        return Err(Error::Truncated);
    }

    Ok(u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

fn read_be_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let end = offset.checked_add(8).ok_or(Error::Truncated)?;

    if end > bytes.len() {
        return Err(Error::Truncated);
    }

    Ok(u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ]))
}