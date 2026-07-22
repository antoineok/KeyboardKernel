//! Keywork Management Protocol (KWMP).
//!
//! The same frames are carried over USB bulk endpoints and a Bluetooth custom
//! GATT service.

mod command;
mod frame;
mod status;
mod wire;

pub use command::ManagementCommand;
pub use frame::{
    FrameDecodeError, FrameFlags, ManagementFrame, ManagementHeader, MANAGEMENT_HEADER_SIZE,
    MANAGEMENT_MAGIC, MANAGEMENT_PROTOCOL_VERSION,
};
pub use status::ManagementStatus;
pub use wire::{WireReadError, WireReader, WireWriteError, WireWriter};
