extern "C" {
    static __heap_start: u8;
    static __heap_end: u8;
}

pub fn heap_bounds() -> (usize, usize) {
    unsafe {
        (
            &__heap_start as *const u8 as usize,
            &__heap_end as *const u8 as usize,
        )
    }
}
use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init_heap() {
    let (start, end) = heap_bounds();
    unsafe {
        ALLOCATOR.lock().init(start as *mut u8, end - start);
    }
    crate::info!(
        "Heap initialized: start=0x{:x}, end=0x{:x} ({} bytes)",
        start,
        end,
        end - start
    );
}
