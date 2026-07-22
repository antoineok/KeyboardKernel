use core::convert::TryFrom;

/// Commands supported by the Keywork Management Protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ManagementCommand {
    // Device.
    GetDeviceInfo = 0x0001,
    GetStatus = 0x0002,
    Reboot = 0x0003,

    // Profiles.
    GetActiveProfile = 0x0100,
    SetActiveProfile = 0x0101,
    ReadProfile = 0x0102,
    WriteProfile = 0x0103,

    // Host -> Keywork assets.
    BeginAssetUpload = 0x0200,
    WriteAssetChunk = 0x0201,
    FinishAssetUpload = 0x0202,
    DeleteAsset = 0x0203,

    // Keywork -> host assets.
    BeginAssetDownload = 0x0300,
    ReadAssetChunk = 0x0301,
    FinishAssetDownload = 0x0302,

    // Firmware update.
    BeginFirmwareUpdate = 0x1000,
    WriteFirmwareChunk = 0x1001,
    FinishFirmwareUpdate = 0x1002,
    AbortFirmwareUpdate = 0x1003,
    GetFirmwareUpdateStatus = 0x1004,
    ConfirmRunningFirmware = 0x1005,
}

impl ManagementCommand {
    pub const fn value(self) -> u16 {
        self as u16
    }
}

impl TryFrom<u16> for ManagementCommand {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let command = match value {
            0x0001 => Self::GetDeviceInfo,
            0x0002 => Self::GetStatus,
            0x0003 => Self::Reboot,
            0x0100 => Self::GetActiveProfile,
            0x0101 => Self::SetActiveProfile,
            0x0102 => Self::ReadProfile,
            0x0103 => Self::WriteProfile,
            0x0200 => Self::BeginAssetUpload,
            0x0201 => Self::WriteAssetChunk,
            0x0202 => Self::FinishAssetUpload,
            0x0203 => Self::DeleteAsset,
            0x0300 => Self::BeginAssetDownload,
            0x0301 => Self::ReadAssetChunk,
            0x0302 => Self::FinishAssetDownload,
            0x1000 => Self::BeginFirmwareUpdate,
            0x1001 => Self::WriteFirmwareChunk,
            0x1002 => Self::FinishFirmwareUpdate,
            0x1003 => Self::AbortFirmwareUpdate,
            0x1004 => Self::GetFirmwareUpdateStatus,
            0x1005 => Self::ConfirmRunningFirmware,
            _ => return Err(()),
        };

        Ok(command)
    }
}
