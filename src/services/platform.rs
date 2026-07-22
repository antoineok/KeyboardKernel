use crate::{
    arch::time::uptime,
    drivers::watchdog::WATCHDOG,
    transport::ActiveTransport,
};

use super::management::{
    AssetId, BeginAssetDownloadResponse, BeginAssetUploadRequest,
    BeginFirmwareUpdateRequest, BeginFirmwareUpdateResponse, BeginTransferResponse,
    DeviceInfo, DeviceStatus, FirmwareSlot, FirmwareUpdateStatus, ManagementBackend,
    ManagementStatus, ProfileWriteOptions, RebootMode, TransferId, CAPABILITY_DISPLAY,
    CAPABILITY_USB,
};

/// Initial kernel binding for KWMP.
///
/// Device/status commands work now. Persistent profile, asset and A/B update
/// operations remain explicitly unsupported until their storage backends are
/// implemented; the USB protocol itself can already return that status.
pub struct KernelManagementBackend {
    active_profile: u8,
    active_transport: ActiveTransport,
}

impl KernelManagementBackend {
    pub const fn new() -> Self {
        Self {
            active_profile: 0,
            active_transport: ActiveTransport::None,
        }
    }

    pub fn set_active_transport(&mut self, transport: ActiveTransport) {
        self.active_transport = transport;
    }
}

impl Default for KernelManagementBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ManagementBackend for KernelManagementBackend {
    fn get_device_info(&mut self) -> Result<DeviceInfo, ManagementStatus> {
        Ok(DeviceInfo {
            hardware_revision: 1,
            firmware_version: 0x0001_0000,
            bootloader_version: 0,
            active_slot: FirmwareSlot::Unknown,
            profile_count: 8,
            capabilities: CAPABILITY_USB | CAPABILITY_DISPLAY,
            serial: *b"KW-0001\0\0\0\0\0\0\0\0\0",
        })
    }

    fn get_status(&mut self) -> Result<DeviceStatus, ManagementStatus> {
        let uptime_ms = uptime().as_millis().min(u128::from(u64::MAX)) as u64;
        let active_transport = match self.active_transport {
            ActiveTransport::None => 0,
            ActiveTransport::Usb => 1,
            ActiveTransport::Bluetooth => 2,
        };
        Ok(DeviceStatus {
            uptime_ms,
            flags: 0,
            active_transport,
            active_profile: self.active_profile,
            active_slot: FirmwareSlot::Unknown,
            pending_slot: FirmwareSlot::Unknown,
            free_storage_bytes: 0,
        })
    }

    fn reboot(&mut self, mode: RebootMode) -> Result<(), ManagementStatus> {
        match mode {
            RebootMode::Normal => WATCHDOG.get().reboot(),
            RebootMode::Bootloader | RebootMode::Recovery => {
                Err(ManagementStatus::Unsupported)
            }
        }
    }

    fn get_active_profile(&mut self) -> Result<u8, ManagementStatus> {
        Ok(self.active_profile)
    }

    fn set_active_profile(&mut self, profile: u8) -> Result<(), ManagementStatus> {
        if profile >= 8 {
            return Err(ManagementStatus::InvalidPayload);
        }
        self.active_profile = profile;
        Ok(())
    }

    fn read_profile(
        &mut self,
        _profile: u8,
        _offset: u32,
        _output: &mut [u8],
    ) -> Result<usize, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn write_profile(
        &mut self,
        _profile: u8,
        _offset: u32,
        _options: ProfileWriteOptions,
        _data: &[u8],
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn begin_asset_upload(
        &mut self,
        _request: BeginAssetUploadRequest,
    ) -> Result<BeginTransferResponse, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn write_asset_chunk(
        &mut self,
        _transfer_id: TransferId,
        _offset: u64,
        _data: &[u8],
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn finish_asset_upload(&mut self, _transfer_id: TransferId) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn delete_asset(&mut self, _asset_id: AssetId) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn begin_asset_download(
        &mut self,
        _asset_id: AssetId,
    ) -> Result<BeginAssetDownloadResponse, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn read_asset_chunk(
        &mut self,
        _transfer_id: TransferId,
        _offset: u64,
        _output: &mut [u8],
    ) -> Result<usize, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn finish_asset_download(
        &mut self,
        _transfer_id: TransferId,
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn begin_firmware_update(
        &mut self,
        _request: BeginFirmwareUpdateRequest<'_>,
    ) -> Result<BeginFirmwareUpdateResponse, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn write_firmware_chunk(
        &mut self,
        _transfer_id: TransferId,
        _offset: u64,
        _data: &[u8],
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn finish_firmware_update(
        &mut self,
        _transfer_id: TransferId,
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn abort_firmware_update(
        &mut self,
        _transfer_id: TransferId,
    ) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn get_firmware_update_status(
        &mut self,
    ) -> Result<FirmwareUpdateStatus, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn confirm_running_firmware(&mut self) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::PermissionDenied)
    }
}
