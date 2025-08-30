//! Entry and exception level transition code for baremetal AArch64.
//!
//! - Executed first in Rust, after `_start` assembly has prepared `.bss`
//!   and passed arguments.
//! - Handles EL2 → EL1 transition and finally runs `main()` in EL1.
use core::arch::global_asm;
use aarch64_cpu::{asm, registers::*};
use tock_registers::interfaces::{Readable, Writeable};

global_asm!(include_str!("boot.s"));

/// The Rust entry of the kernel binary.
///
/// Called from `_start` (assembly).
///
/// # Safety
/// - Must be called from EL2.
/// - `stack_end_exclusive_addr` must point to valid RAM reserved for the EL1 stack.
/// - `fdt_ptr` must be the DTB pointer provided by the firmware.
#[no_mangle]
pub unsafe extern "C" fn _start_rust(
    stack_end_exclusive_addr: u64,
    fdt_ptr: u64,
) -> ! {
    // Stash DTB pointer somewhere global if you need it later in EL1.
    FDT_PTR = fdt_ptr;

    prepare_el2_to_el1_transition(stack_end_exclusive_addr);

    // Drop to EL1 at 'main(fdt_ptr)'
    asm::eret();
}

/// Global storage for the DTB pointer (accessible from EL1).
#[no_mangle]
pub static mut FDT_PTR: u64 = 0;

/// Prepares the transition from EL2 to EL1.
///
/// # Safety
///
/// - BSS must already be cleared (done in assembly).
/// - Stack memory must be valid.
#[inline(always)]
unsafe fn prepare_el2_to_el1_transition(stack_end_exclusive_addr: u64) {
    // Enable timer counter registers for EL1.
    CNTHCTL_EL2.write(
        CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET
    );

    // No virtual offset for the counter.
    CNTVOFF_EL2.set(0);

    // EL1 execution state = AArch64.
    HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);

    // Fake a saved program status for the exception return:
    // - Interrupts masked
    // - Mode = EL1h (use SP_EL1)
    SPSR_EL2.write(
        SPSR_EL2::D::Masked
            + SPSR_EL2::A::Masked
            + SPSR_EL2::I::Masked
            + SPSR_EL2::F::Masked
            + SPSR_EL2::M::EL1h,
    );

    // Link register: where to continue at EL1. We want main(fdt_ptr).
    ELR_EL2.set(crate::main as *const () as u64);

    // Set up SP_EL1 (stack pointer for EL1).
    SP_EL1.set(stack_end_exclusive_addr);
}