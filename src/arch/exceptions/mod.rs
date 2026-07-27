//--------------------------------------------------------------------------------------------------
// Architectural Public Reexports
//--------------------------------------------------------------------------------------------------
use core::arch::global_asm;

use aarch64_cpu::registers::*;
use tock_registers::interfaces::Readable;

global_asm!(include_str!("el1.s"));

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Kernel privilege levels.
#[allow(missing_docs)]
#[derive(Eq, PartialEq)]
pub enum PrivilegeLevel {
    User,
    Kernel,
    Hypervisor,
    Unknown,
}

#[repr(C)]
pub struct Registers {
    pub x: [u64; 31], // x0-x30 (x30 is LR)
    pub spsr_el1: u64,
    pub elr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
}

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

#[inline(always)]
pub fn unmask_irq() {
    unsafe {
        core::arch::asm!(
            "msr daifclr, #2",
            "isb",
            options(nostack, preserves_flags),
        );
    }
}

#[inline(always)]
pub fn mask_irq() {
    unsafe {
        core::arch::asm!(
            "msr daifset, #2",
            "isb",
            options(nostack, preserves_flags),
        );
    }
}

fn handle_irq() -> bool {
    let Some(ack) = crate::arch::gic::GIC_PLATFORM.acknowledge() else {
        // GICv2 returns a spurious INTID when the vector was taken without a
        // deliverable interrupt. There is nothing to EOI in that case.
        return true;
    };

    let handled = if ack.intid == crate::video::hevc::HEVC_PLATFORM.gic_intid() {
        crate::video::hevc::HEVC_PLATFORM.handle_irq();
        true
    } else {
        false
    };

    // Clear the peripheral source before dropping the GIC active state.
    crate::arch::gic::GIC_PLATFORM.end_interrupt(ack);
    handled
}

impl PrivilegeLevel {
    pub fn is_kernel(&self) -> bool {
        *self == PrivilegeLevel::Kernel
    }

    pub fn is_user(&self) -> bool {
        *self == PrivilegeLevel::User
    }

    pub fn is_hypervisor(&self) -> bool {
        *self == PrivilegeLevel::Hypervisor
    }

    pub fn is_unknown(&self) -> bool {
        *self == PrivilegeLevel::Unknown
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PrivilegeLevel::User => "EL0",
            PrivilegeLevel::Kernel => "EL1",
            PrivilegeLevel::Hypervisor => "EL2",
            PrivilegeLevel::Unknown => "Unknown",
        }
    }

    pub fn current() -> Self {
        let el = CurrentEL.read_as_enum(CurrentEL::EL);
        if el.is_none() {
            return PrivilegeLevel::Unknown;
        }
        match el.unwrap() {
            CurrentEL::EL::Value::EL2 => PrivilegeLevel::Hypervisor,
            CurrentEL::EL::Value::EL1 => PrivilegeLevel::Kernel,
            CurrentEL::EL::Value::EL0 => PrivilegeLevel::User,
            _ => PrivilegeLevel::Unknown,
        }
    }
}

#[no_mangle]
pub extern "C" fn el1_handler(exception_number: u64, sp: u64, regs: *const Registers) {
    if matches!(exception_number, 1 | 5) && handle_irq() {
        return;
    }

    use crate::{error, info, warn};
    let regs = unsafe { &*regs };

    match exception_number {
        0 => info!("EL1 Exception: Synchronous from SP0"),
        1 => warn!("EL1 Exception: IRQ from SP0"),
        2 => error!("EL1 Exception: FIQ from SP0"),
        3 => error!("EL1 Exception: SError from SP0"),
        4 => info!("EL1 Exception: Synchronous from SPx"),
        5 => warn!("EL1 Exception: IRQ from SPx"),
        6 => error!("EL1 Exception: FIQ from SPx"),
        7 => error!("EL1 Exception: SError from SPx"),
        _ => error!("EL1 Exception: Unknown vector {}", exception_number),
    }

    info!("SP:   {:#018x}", sp);
    info!("SPSR: {:#018x}", regs.spsr_el1);
    info!("ELR:  {:#018x}", regs.elr_el1);
    info!("ESR:  {:#018x}", regs.esr_el1);
    info!("FAR:  {:#018x}", regs.far_el1);

    for (i, reg) in regs.x.iter().enumerate() {
        info!("x{:02}: {:#018x}", i, reg);
    }

    #[cfg(feature = "jtag")]
    crate::jtag::breakpoint();

    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) }
    }
}
