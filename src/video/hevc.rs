use core::cell::UnsafeCell;

use crate::{drivers::mailbox::MailboxBuffer, fdt::{AliasEntry, DeviceEntry}, info};

pub const HEVC_COMPATIBLE: &str = "brcm,bcm2711-hevc-dec";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GicInterruptKind {
    Spi,
    Ppi,
}

impl GicInterruptKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spi => "SPI",
            Self::Ppi => "PPI",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GicInterrupt {
    pub kind: GicInterruptKind,
    pub number: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockSpecifier {
    pub provider_phandle: u32,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HevcResources {
    pub hevc: AliasEntry,
    pub intc: AliasEntry,
    pub interrupt: GicInterrupt,
    pub clock: ClockSpecifier,
}

pub struct HevcPlatform(UnsafeCell<Option<HevcResources>>);

unsafe impl Sync for HevcPlatform {}

pub static HEVC_PLATFORM: HevcPlatform = HevcPlatform(UnsafeCell::new(None));

impl HevcPlatform {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(None))
    }

    pub fn init(&self, device: Option<DeviceEntry>) {
        let device = device.unwrap_or_else(|| {
            panic!("BCM2711 HEVC decoder not found in device tree")
        });

        let hevc = device
            .resource("hevc")
            .unwrap_or_else(|| panic!("BCM2711 HEVC decoder has no 'hevc' register resource"));
        let intc = device
            .resource("intc")
            .unwrap_or_else(|| panic!("BCM2711 HEVC decoder has no 'intc' register resource"));

        assert_eq!(hevc.arm_phys & 0xfff, 0, "HEVC register base is not page aligned");
        assert_eq!(intc.arm_phys & 0xfff, 0, "HEVC interrupt-controller base is not page aligned");

        let interrupt_cells = device.interrupts();
        assert_eq!(
            interrupt_cells.len(),
            3,
            "BCM2711 HEVC decoder must expose one three-cell GIC interrupt"
        );
        let interrupt = GicInterrupt {
            kind: match interrupt_cells[0] {
                0 => GicInterruptKind::Spi,
                1 => GicInterruptKind::Ppi,
                kind => panic!("unsupported HEVC GIC interrupt type {kind}"),
            },
            number: interrupt_cells[1],
            flags: interrupt_cells[2],
        };

        let clock_cells = device.clocks();
        assert_eq!(
            clock_cells.len(),
            2,
            "BCM2711 HEVC decoder must expose one clock specifier"
        );
        let clock = ClockSpecifier {
            provider_phandle: clock_cells[0],
            id: clock_cells[1],
        };

        unsafe {
            let slot = &mut *self.0.get();
            assert!(slot.is_none(), "HEVC platform resources already initialized");
            *slot = Some(HevcResources {
                hevc,
                intc,
                interrupt,
                clock,
            });
        }

        info!(
            "HEVC decoder discovered: hevc bus={:#018x} phys={:#018x} size={} KiB, intc bus={:#018x} phys={:#018x} size={} KiB, irq={} {} flags={:#x}, clock=phandle {:#x} id={}",
            hevc.vc_addr,
            hevc.arm_phys,
            hevc.size / 1024,
            intc.vc_addr,
            intc.arm_phys,
            intc.size / 1024,
            interrupt.kind.as_str(),
            interrupt.number,
            interrupt.flags,
            clock.provider_phandle,
            clock.id,
        );
    }

    pub fn enable_clock(&self, mailbox: &mut MailboxBuffer) -> u32 {
        let resources = self.resources();
        let clock_id = resources.clock.id;

        let max_rate = mailbox
            .get_clock_max_rate(clock_id)
            .unwrap_or_else(|| panic!("failed to query HEVC clock {clock_id} maximum rate"));
        let actual_rate = mailbox
            .set_clock_rate(clock_id, max_rate)
            .unwrap_or_else(|| panic!("failed to set HEVC clock {clock_id} rate"));

        assert!(
            mailbox.set_clock_state(clock_id, true),
            "failed to enable HEVC clock {clock_id}"
        );

        info!(
            "HEVC clock enabled: id={} requested={} Hz actual={} Hz",
            clock_id, max_rate, actual_rate
        );

        actual_rate
    }

    pub fn probe_hardware(&self) -> u32 {
        super::rpivid::RpividRegisters::from_resources(self.resources()).probe()
    }

    pub fn resources(&self) -> HevcResources {
        unsafe {
            (*self.0.get())
                .expect("HEVC platform resources requested before initialization")
        }
    }
}
