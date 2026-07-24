use core::{arch::asm, fmt};

pub const CPU_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct CpuId(u8);

impl CpuId {
    pub const BOOT: Self = Self(0);

    pub const fn new(raw: u8) -> Option<Self> {
        if (raw as usize) < CPU_COUNT {
            Some(Self(raw))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn current() -> Self {
        let mpidr: u64;

        unsafe {
            asm!(
                "mrs {mpidr}, mpidr_el1",
                mpidr = out(reg) mpidr,
                options(nomem, nostack, preserves_flags),
            );
        }

        let affinity0 = (mpidr & 0xff) as u8;

        Self::new(affinity0)
            .unwrap_or_else(|| panic!("unsupported CPU affinity level 0 ID: {affinity0}"))
    }

    #[inline(always)]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[inline(always)]
    pub const fn raw(self) -> u8 {
        self.0
    }

    #[inline(always)]
    pub const fn is_boot(self) -> bool {
        self.0 == Self::BOOT.0
    }
}

impl fmt::Display for CpuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CPU{}", self.0)
    }
}

#[inline(always)]
pub fn current_id() -> CpuId {
    CpuId::current()
}
