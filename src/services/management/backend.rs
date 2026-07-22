use super::{
    DeviceInfo, DeviceStatus, ManagementStatus, ProfileWriteOptions, RebootMode,
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
}
