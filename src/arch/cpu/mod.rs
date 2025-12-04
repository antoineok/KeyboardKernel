use core::arch::global_asm;

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
    fn el1_drop(device_tree_ptr: usize);
    fn park() -> !;
}

#[no_mangle]
pub extern "C" fn el2_start(device_tree_ptr: usize) -> ! {
    unsafe {
        el1_drop(device_tree_ptr);

        park()
    }
}
