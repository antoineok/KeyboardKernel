use core::cell::UnsafeCell;
use crate::storage::block::EmmcBlockDevice;
use crate::storage::partition::PartitionBlockDevice; // whatever your slice type is

pub struct DevStatic<T>(UnsafeCell<T>);
unsafe impl<T> Sync for DevStatic<T> {}

impl<T> DevStatic<T> {
    pub const fn new(v: T) -> Self { Self(UnsafeCell::new(v)) }
    pub fn get(&self) -> &T { unsafe { &*self.0.get() } }
    pub fn get_mut(&self) -> &mut T { unsafe { &mut *self.0.get() } }
}

// Base disk
pub static EMMC_DEV: DevStatic<EmmcBlockDevice> =
    DevStatic::new(EmmcBlockDevice::new());

// Partition slices (store Option until initialized)
pub static BOOT_SLICE: DevStatic<Option<PartitionBlockDevice<'static>>> =
    DevStatic::new(None);

pub static ROM_SLICE: DevStatic<Option<PartitionBlockDevice<'static>>> =
    DevStatic::new(None);
