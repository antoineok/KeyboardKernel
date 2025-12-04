use fdt::Fdt;

use crate::{drivers::uart::UART, fdt::MmioTable, info};
use core::{arch::global_asm, mem::MaybeUninit};
global_asm!(include_str!("boot.s"));
global_asm!(include_str!("el2_drop.s"));
global_asm!(
    "
    .global park
    park:
        wfi
        b park
"
);
extern "C" {
    fn el1_drop();
    fn park() -> !;
}

static mut MMIO_GLOBAL: MaybeUninit<MmioTable<'static>> = MaybeUninit::uninit();

pub static DTB_ADDR: core::sync::atomic::AtomicPtr<()> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

#[no_mangle]
pub extern "C" fn el2_start(device_tree_ptr: usize) -> ! {
    // Make Fdt<'static> — blob really is static in kernel boot
    let fdt = unsafe { Fdt::from_ptr(device_tree_ptr as *const u8).unwrap() };
    let fdt: Fdt<'static> = unsafe { core::mem::transmute(fdt) };

    // Build table using fdt with the same lifetime 'static
    let mmio_table = MmioTable::from_fdt(&fdt);

    unsafe {
        MMIO_GLOBAL.write(mmio_table);
        let ptr_static = MMIO_GLOBAL.assume_init_ref() as *const _;

        // Store as *mut () (type erased pointer)
        DTB_ADDR.store(ptr_static as *mut (), core::sync::atomic::Ordering::Relaxed);
    }

    let mmio_table = get_mmio_table();

    let uart = UART.get();
    uart.init(mmio_table.entries.get("aux"));
    info!("Logger initialized");

    mmio_table.log();

    #[cfg(feature = "jtag")]
    {
        crate::jtag::init_jtag();
        crate::info!("JTAG initialized");
    }
    unsafe {
        el1_drop();

        park()
    }
}

pub fn get_mmio_table() -> &'static MmioTable<'static> {
    let ptr = DTB_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    assert!(!ptr.is_null());
    unsafe { &*(ptr as *const MmioTable<'static>) }
}
