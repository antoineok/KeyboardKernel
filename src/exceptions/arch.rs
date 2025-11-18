// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2018-2023 Andre Richter <andre.o.richter@gmail.com>

//! Architectural synchronous and asynchronous exception handling.
//!
//! # Orientation
//!
//! Since arch modules are imported into generic modules using the path attribute, the path of this
//! file is:
//!
//! crate::exception::arch_exception

use core::arch::global_asm;

use aarch64_cpu::registers::*;
use tock_registers::interfaces::Readable;

use crate::{error, info, warn};

#[repr(C)]
pub struct Registers {
    pub x: [u64; 31], // x0-x30 (x30 is LR)
    pub spsr_el1: u64,
    pub elr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
}

global_asm!(include_str!("el1.s"));

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------
use crate::exceptions::PrivilegeLevel;

/// The processing element's current privilege level.
pub fn current_privilege_level() -> (PrivilegeLevel, &'static str) {
    let el = CurrentEL.read_as_enum(CurrentEL::EL);
    match el {
        Some(CurrentEL::EL::Value::EL2) => (PrivilegeLevel::Hypervisor, "EL2"),
        Some(CurrentEL::EL::Value::EL1) => (PrivilegeLevel::Kernel, "EL1"),
        Some(CurrentEL::EL::Value::EL0) => (PrivilegeLevel::User, "EL0"),
        _ => (PrivilegeLevel::Unknown, "Unknown"),
    }
}

#[no_mangle]
pub extern "C" fn el1_handler(exception_number: u64, sp: u64, regs: *const Registers) {
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
