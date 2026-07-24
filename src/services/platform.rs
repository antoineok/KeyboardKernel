extern crate alloc;

use alloc::vec::Vec;
use core::{convert::TryFrom, time::Duration};

use crate::{
    arch::time::uptime,
    drivers::watchdog::WATCHDOG,
    transport::ActiveTransport,
    update::{kernel::KernelUpdater, sha256::sha256, UpdateError},
};

use super::management::{
    AssetId, BeginAssetDownloadResponse, BeginAssetUploadRequest, BeginFirmwareUpdateRequest,
    BeginFirmwareUpdateResponse, BeginTransferResponse, DeviceInfo, DeviceStatus, FirmwareSlot,
    FirmwareUpdatePhase, FirmwareUpdateStatus, ManagementBackend, ManagementStatus,
    ProfileWriteOptions, RebootMode, TransferId, CAPABILITY_DISPLAY, CAPABILITY_FIRMWARE_UPDATE,
    CAPABILITY_USB,
};

const MAX_FIRMWARE_IMAGE_SIZE: usize = 3 * 1024 * 1024;
const MAX_FIRMWARE_CHUNK_SIZE: usize = 3072;
const REBOOT_RESPONSE_GRACE: Duration = Duration::from_millis(250);

struct FirmwareTransfer {
    phase: FirmwareUpdatePhase,
    transfer_id: Option<TransferId>,
    next_transfer_id: u32,
    expected_size: usize,
    expected_sha256: [u8; 32],
    bytes_received: usize,
    image: Vec<u8>,
    last_error: u16,
}

impl FirmwareTransfer {
    const fn new() -> Self {
        Self {
            phase: FirmwareUpdatePhase::Idle,
            transfer_id: None,
            next_transfer_id: 1,
            expected_size: 0,
            expected_sha256: [0; 32],
            bytes_received: 0,
            image: Vec::new(),
            last_error: ManagementStatus::Ok as u16,
        }
    }

    fn begin(
        &mut self,
        request: BeginFirmwareUpdateRequest<'_>,
    ) -> Result<BeginFirmwareUpdateResponse, ManagementStatus> {
        if !matches!(
            self.phase,
            FirmwareUpdatePhase::Idle | FirmwareUpdatePhase::Failed
        ) {
            return Err(ManagementStatus::Busy);
        }
        if !request.signature.is_empty() {
            return Err(ManagementStatus::InvalidSignature);
        }

        let image_size =
            usize::try_from(request.image_size).map_err(|_| ManagementStatus::InvalidFirmware)?;
        if !(4096..=MAX_FIRMWARE_IMAGE_SIZE).contains(&image_size) {
            return Err(ManagementStatus::InvalidFirmware);
        }

        KernelUpdater::discard_staged().map_err(map_update_error)?;

        self.image.clear();
        self.image
            .try_reserve_exact(image_size)
            .map_err(|_| ManagementStatus::NoSpace)?;

        let transfer_id = TransferId(self.next_transfer_id);
        self.next_transfer_id = self.next_transfer_id.wrapping_add(1).max(1);
        self.phase = FirmwareUpdatePhase::Receiving;
        self.transfer_id = Some(transfer_id);
        self.expected_size = image_size;
        self.expected_sha256 = request.sha256;
        self.bytes_received = 0;
        self.last_error = ManagementStatus::Ok.value();

        Ok(BeginFirmwareUpdateResponse {
            transfer_id,
            maximum_chunk_size: MAX_FIRMWARE_CHUNK_SIZE as u16,
            target_slot: FirmwareSlot::Unknown,
        })
    }

    fn write_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ManagementStatus> {
        if self.phase != FirmwareUpdatePhase::Receiving {
            return Err(ManagementStatus::InvalidState);
        }
        if self.transfer_id != Some(transfer_id) {
            return Err(ManagementStatus::InvalidTransfer);
        }
        if data.is_empty() || data.len() > MAX_FIRMWARE_CHUNK_SIZE {
            return Err(ManagementStatus::InvalidPayload);
        }
        if offset != self.bytes_received as u64 {
            return Err(ManagementStatus::InvalidOffset);
        }
        if self.bytes_received.saturating_add(data.len()) > self.expected_size {
            return Err(ManagementStatus::InvalidOffset);
        }

        self.image.extend_from_slice(data);
        self.bytes_received += data.len();
        Ok(())
    }

    fn finish(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus> {
        if self.phase != FirmwareUpdatePhase::Receiving {
            return Err(ManagementStatus::InvalidState);
        }
        if self.transfer_id != Some(transfer_id) {
            return Err(ManagementStatus::InvalidTransfer);
        }
        if self.bytes_received != self.expected_size {
            return self.fail(ManagementStatus::InvalidState);
        }

        self.phase = FirmwareUpdatePhase::Verifying;
        if sha256(&self.image) != self.expected_sha256 {
            return self.fail(ManagementStatus::ChecksumMismatch);
        }

        if let Err(error) = KernelUpdater::stage(&self.image) {
            return self.fail(map_update_error(error));
        }

        // Free the upload buffer before re-reading the staged file for an
        // independent on-storage verification. The kernel heap is currently
        // only 4 MiB.
        self.image = Vec::new();

        if let Err(error) = KernelUpdater::verify_staged(self.expected_size, &self.expected_sha256)
        {
            return self.fail(map_update_error(error));
        }

        self.phase = FirmwareUpdatePhase::PendingReboot;
        self.last_error = ManagementStatus::Ok.value();
        Ok(())
    }

    fn abort(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus> {
        if self.transfer_id != Some(transfer_id) {
            return Err(ManagementStatus::InvalidTransfer);
        }

        self.reset();
        KernelUpdater::discard_staged().map_err(map_update_error)
    }

    fn status(&self) -> FirmwareUpdateStatus {
        FirmwareUpdateStatus {
            phase: self.phase,
            transfer_id: self.transfer_id,
            target_slot: FirmwareSlot::Unknown,
            bytes_received: self.bytes_received as u64,
            total_size: self.expected_size as u64,
            last_error: self.last_error,
        }
    }

    fn reset(&mut self) {
        self.phase = FirmwareUpdatePhase::Idle;
        self.transfer_id = None;
        self.expected_size = 0;
        self.expected_sha256 = [0; 32];
        self.bytes_received = 0;
        self.image = Vec::new();
        self.last_error = ManagementStatus::Ok.value();
    }

    fn fail<T>(&mut self, status: ManagementStatus) -> Result<T, ManagementStatus> {
        self.phase = FirmwareUpdatePhase::Failed;
        self.last_error = status.value();
        Err(status)
    }

    fn is_pending_reboot(&self) -> bool {
        self.phase == FirmwareUpdatePhase::PendingReboot && KernelUpdater::has_staged()
    }
}

/// Kernel binding for the transport-neutral KWMP dispatcher.
pub struct KernelManagementBackend {
    active_transport: ActiveTransport,
    firmware: FirmwareTransfer,
    reboot_at: Option<Duration>,
    commit_update_on_reboot: bool,
}

impl KernelManagementBackend {
    pub const fn new() -> Self {
        Self {
            active_transport: ActiveTransport::None,
            firmware: FirmwareTransfer::new(),
            reboot_at: None,
            commit_update_on_reboot: false,
        }
    }

    pub fn set_active_transport(&mut self, transport: ActiveTransport) {
        self.active_transport = transport;
    }

    /// Execute a reboot only after the small KWMP response has had time to
    /// leave the bulk IN endpoint.
    pub fn poll_pending_action(&mut self) {
        let Some(deadline) = self.reboot_at else {
            return;
        };
        if uptime() < deadline {
            return;
        }

        self.reboot_at = None;
        if self.commit_update_on_reboot {
            KernelUpdater::commit_and_reboot();
        }
        WATCHDOG.get().reboot();
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
            firmware_version: 0x0001_0003,
            bootloader_version: 0,
            active_slot: FirmwareSlot::Unknown,
            profile_count: 0,
            capabilities: CAPABILITY_USB | CAPABILITY_DISPLAY | CAPABILITY_FIRMWARE_UPDATE,
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
            active_profile: 0xff,
            active_slot: FirmwareSlot::Unknown,
            pending_slot: FirmwareSlot::Unknown,
            free_storage_bytes: 0,
        })
    }

    fn reboot(&mut self, mode: RebootMode) -> Result<(), ManagementStatus> {
        match mode {
            RebootMode::Normal => {
                self.commit_update_on_reboot = self.firmware.is_pending_reboot();
                self.reboot_at = Some(uptime() + REBOOT_RESPONSE_GRACE);
                Ok(())
            }
            RebootMode::Bootloader | RebootMode::Recovery => Err(ManagementStatus::Unsupported),
        }
    }

    fn get_active_profile(&mut self) -> Result<u8, ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn set_active_profile(&mut self, _profile: u8) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
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

    fn finish_asset_download(&mut self, _transfer_id: TransferId) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::Unsupported)
    }

    fn begin_firmware_update(
        &mut self,
        request: BeginFirmwareUpdateRequest<'_>,
    ) -> Result<BeginFirmwareUpdateResponse, ManagementStatus> {
        self.firmware.begin(request)
    }

    fn write_firmware_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
        data: &[u8],
    ) -> Result<(), ManagementStatus> {
        self.firmware.write_chunk(transfer_id, offset, data)
    }

    fn finish_firmware_update(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus> {
        self.firmware.finish(transfer_id)
    }

    fn abort_firmware_update(&mut self, transfer_id: TransferId) -> Result<(), ManagementStatus> {
        self.firmware.abort(transfer_id)
    }

    fn get_firmware_update_status(&mut self) -> Result<FirmwareUpdateStatus, ManagementStatus> {
        Ok(self.firmware.status())
    }

    fn confirm_running_firmware(&mut self) -> Result<(), ManagementStatus> {
        Err(ManagementStatus::PermissionDenied)
    }
}

fn map_update_error(error: UpdateError) -> ManagementStatus {
    match error {
        UpdateError::InvalidImage | UpdateError::BadMagic => ManagementStatus::InvalidFirmware,
        UpdateError::VerifyFailed | UpdateError::CrcFailed => ManagementStatus::ChecksumMismatch,
        UpdateError::MissingStagedImage | UpdateError::NoActiveSession => {
            ManagementStatus::InvalidState
        }
        UpdateError::Alloc | UpdateError::TooMuchData => ManagementStatus::NoSpace,
        UpdateError::BadSequence => ManagementStatus::InvalidOffset,
        UpdateError::BadPacket | UpdateError::UnexpectedEnd => ManagementStatus::InvalidPayload,
        UpdateError::Vfs(_) | UpdateError::BadCommand => ManagementStatus::InternalError,
    }
}
