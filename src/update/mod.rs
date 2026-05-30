pub mod crc32;
pub mod error;
pub mod kernel;
pub mod uart;

pub use error::UpdateError;
pub use kernel::KernelUpdater;

#[used]
#[link_section = ".update_magic"]
pub(in crate::update) static UPDATE_MAGIC: u32 = 0xDEAD_FEED;