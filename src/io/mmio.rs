#[derive(Copy, Clone)]
pub struct MmioAddress(pub usize);

impl MmioAddress {
    #[inline(always)]
    pub fn read(self) -> u32 {
        unsafe { core::ptr::read_volatile(self.0 as *const u32) }
    }

    #[inline(always)]
    pub fn write(self, val: u32) {
        unsafe { core::ptr::write_volatile(self.0 as *mut u32, val) }
    }

    pub const fn offset(self, offset: usize) -> MmioAddress {
        MmioAddress(self.0 + offset)
    }

    pub fn set_address(&mut self, addr: u64) {
        self.0 = addr as usize;
    }
}
