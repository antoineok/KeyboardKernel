//! Transport-independent application services.

pub mod management;
mod platform;
mod runtime;

pub use platform::KernelManagementBackend;
pub use runtime::KeyworkServices;
