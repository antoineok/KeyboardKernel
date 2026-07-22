//! Keywork Management Protocol (KWMP).
//!
//! The same frames are carried over USB bulk endpoints and a Bluetooth custom
//! GATT service.

mod backend;
mod command;
mod frame;
mod model;
mod service;
mod status;
mod wire;

pub use backend::ManagementBackend;
pub use command::ManagementCommand;
pub use frame::{
    FrameDecodeError, FrameFlags, ManagementFrame, ManagementHeader, MANAGEMENT_HEADER_SIZE,
    MANAGEMENT_MAGIC, MANAGEMENT_PROTOCOL_VERSION,
};
pub use model::{
    DeviceInfo, DeviceStatus, FirmwareSlot, ProfileWriteOptions, RebootMode, CAPABILITY_AB_UPDATE,
    CAPABILITY_ASSET_DOWNLOAD, CAPABILITY_ASSET_UPLOAD, CAPABILITY_BLUETOOTH, CAPABILITY_DISPLAY,
    CAPABILITY_USB,
};
pub use service::{ManagementService, ManagementServiceError};
pub use status::ManagementStatus;
pub use wire::{WireReadError, WireReader, WireWriteError, WireWriter};
