use super::{WireWriteError, WireWriter};

pub const CAPABILITY_USB: u64 = 1 << 0;
pub const CAPABILITY_BLUETOOTH: u64 = 1 << 1;
pub const CAPABILITY_AB_UPDATE: u64 = 1 << 2;
pub const CAPABILITY_ASSET_UPLOAD: u64 = 1 << 3;
pub const CAPABILITY_ASSET_DOWNLOAD: u64 = 1 << 4;
pub const CAPABILITY_DISPLAY: u64 = 1 << 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FirmwareSlot {
    A = 0,
    B = 1,
    Unknown = 0xff,
}

impl FirmwareSlot {
    pub const fn value(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RebootMode {
    Normal = 0,
    Bootloader = 1,
    Recovery = 2,
}

impl RebootMode {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Normal),
            1 => Some(Self::Bootloader),
            2 => Some(Self::Recovery),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    pub hardware_revision: u16,
    pub firmware_version: u32,
    pub bootloader_version: u32,
    pub active_slot: FirmwareSlot,
    pub profile_count: u8,
    pub capabilities: u64,
    pub serial: [u8; 16],
}

impl DeviceInfo {
    pub const ENCODED_LEN: usize = 40;

    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u16(super::MANAGEMENT_PROTOCOL_VERSION as u16)?;
        writer.write_u16(self.hardware_revision)?;
        writer.write_u32(self.firmware_version)?;
        writer.write_u32(self.bootloader_version)?;
        writer.write_u8(self.active_slot.value())?;
        writer.write_u8(self.profile_count)?;
        writer.write_u16(0)?;
        writer.write_u64(self.capabilities)?;
        writer.write_bytes(&self.serial)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceStatus {
    pub uptime_ms: u64,
    pub flags: u32,
    pub active_transport: u8,
    pub active_profile: u8,
    pub active_slot: FirmwareSlot,
    pub pending_slot: FirmwareSlot,
    pub free_storage_bytes: u64,
}

impl DeviceStatus {
    pub const ENCODED_LEN: usize = 24;

    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u64(self.uptime_ms)?;
        writer.write_u32(self.flags)?;
        writer.write_u8(self.active_transport)?;
        writer.write_u8(self.active_profile)?;
        writer.write_u8(self.active_slot.value())?;
        writer.write_u8(self.pending_slot.value())?;
        writer.write_u64(self.free_storage_bytes)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileWriteOptions {
    pub truncate_before_write: bool,
    pub final_chunk: bool,
}

impl ProfileWriteOptions {
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !0b11 != 0 {
            return None;
        }

        Some(Self {
            truncate_before_write: bits & (1 << 0) != 0,
            final_chunk: bits & (1 << 1) != 0,
        })
    }
}
