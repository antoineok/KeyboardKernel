use super::{
    AssetId, BeginAssetDownloadResponse, BeginAssetUploadRequest, BeginFirmwareUpdateRequest,
    BeginFirmwareUpdateResponse, BeginTransferResponse, DeviceInfo, DeviceStatus,
    FirmwareUpdateStatus, ManagementStatus, ProfileWriteOptions, RebootMode, TransferId,
};

/// Platform-specific operations used by the transport-neutral management
/// dispatcher.
pub trait ManagementBackend {
    fn get_device_info(&mut self) -> Result<DeviceInfo, ManagementStatus>;

    fn get_status(&mut self) -> Result<DeviceStatus, ManagementStatus>;

    fn reboot(&mut self, mode: RebootMode) -> Result<(), ManagementStatus>;

    fn get_active_profile(&mut self) -> Result<u8, ManagementStatus>;

    fn set_active_profile(&mut self, profile: u8) -> Result<(), ManagementStatus>;

    /// Reads up to `output.len()` bytes starting at `offset`.
    fn read_profile(
        &mut self,
        profile: u8,
        offset: u32,
        output: &mut [u8],
    ) -> Result<usize, ManagementStatus>;

    /// Writes a profile chunk. The backend decides how profiles are persisted
    /// and atomically published after the final chunk.
    fn write_profile(
        &mut self,
        profile: u8,
        offset: u32,
        options: ProfileWriteOptions,
        data: &[u8],
    ) -> Result<(), ManagementStatus>;

    fn begin_asset_upload(
        &mut self,
        request: BeginAssetUploadRequest,
    ) -> Result<BeginTransferResponse, ManagementStatus>;

    fn write_asset_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ManagementStatus>;

    fn finish_asset_upload(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus>;

    fn delete_asset(&mut self, asset_id: AssetId) -> Result<(), ManagementStatus>;

    fn begin_asset_download(
        &mut self,
        asset_id: AssetId,
    ) -> Result<BeginAssetDownloadResponse, ManagementStatus>;

    fn read_asset_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, ManagementStatus>;

    fn finish_asset_download(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus>;

    fn begin_firmware_update(
        &mut self,
        request: BeginFirmwareUpdateRequest<'_>,
    ) -> Result<BeginFirmwareUpdateResponse, ManagementStatus>;

    fn write_firmware_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ManagementStatus>;

    fn finish_firmware_update(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus>;

    fn abort_firmware_update(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus>;

    fn get_firmware_update_status(&mut self) -> Result<FirmwareUpdateStatus, ManagementStatus>;

    /// Confirmation should normally be initiated by healthy newly-booted
    /// firmware. A production backend may reject external confirmation with
    /// `PermissionDenied` unless a physical/developer authorization is active.
    fn confirm_running_firmware(&mut self) -> Result<(), ManagementStatus>;
}
