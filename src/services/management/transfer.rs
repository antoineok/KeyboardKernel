use super::{FirmwareSlot, WireWriteError, WireWriter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct TransferId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AssetId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AssetKind {
    Image = 0,
    Animation = 1,
    Font = 2,
    Theme = 3,
    Other = 0xff,
}

impl AssetKind {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Image),
            1 => Some(Self::Animation),
            2 => Some(Self::Font),
            3 => Some(Self::Theme),
            0xff => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginAssetUploadRequest {
    pub asset_id: AssetId,
    pub kind: AssetKind,
    pub total_size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginTransferResponse {
    pub transfer_id: TransferId,
    pub maximum_chunk_size: u16,
}

impl BeginTransferResponse {
    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u32(self.transfer_id.0)?;
        writer.write_u16(self.maximum_chunk_size)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginAssetDownloadResponse {
    pub transfer_id: TransferId,
    pub total_size: u64,
    pub maximum_chunk_size: u16,
    pub sha256: [u8; 32],
}

impl BeginAssetDownloadResponse {
    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u32(self.transfer_id.0)?;
        writer.write_u64(self.total_size)?;
        writer.write_u16(self.maximum_chunk_size)?;
        writer.write_bytes(&self.sha256)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginFirmwareUpdateRequest<'a> {
    pub image_size: u64,
    pub image_version: u32,
    pub sha256: [u8; 32],
    pub signature: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginFirmwareUpdateResponse {
    pub transfer_id: TransferId,
    pub maximum_chunk_size: u16,
    pub target_slot: FirmwareSlot,
}

impl BeginFirmwareUpdateResponse {
    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u32(self.transfer_id.0)?;
        writer.write_u16(self.maximum_chunk_size)?;
        writer.write_u8(self.target_slot.value())?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FirmwareUpdatePhase {
    Idle = 0,
    Preparing = 1,
    Receiving = 2,
    Verifying = 3,
    PendingReboot = 4,
    Failed = 5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirmwareUpdateStatus {
    pub phase: FirmwareUpdatePhase,
    pub transfer_id: Option<TransferId>,
    pub target_slot: FirmwareSlot,
    pub bytes_received: u64,
    pub total_size: u64,
    pub last_error: u16,
}

impl FirmwareUpdateStatus {
    pub fn encode(self, writer: &mut WireWriter<'_>) -> Result<(), WireWriteError> {
        writer.write_u8(self.phase as u8)?;
        writer.write_u8(self.target_slot.value())?;
        writer.write_u16(self.last_error)?;
        writer.write_u32(self.transfer_id.map_or(0, |id| id.0))?;
        writer.write_u64(self.bytes_received)?;
        writer.write_u64(self.total_size)?;
        Ok(())
    }
}
