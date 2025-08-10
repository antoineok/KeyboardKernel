use core::arch::asm;

pub fn wait_msec(wait_time: u32) {
    let mut freq: u64;
    let mut start: u64;
    let mut now: u64;

    unsafe {
        // Read the counter frequency (Hz)
        asm!("mrs {0}, cntfrq_el0", out(reg) freq);
        // Read the current counter value
        asm!("mrs {0}, cntpct_el0", out(reg) start);

        // Calculate expire value for counter
        let expire = start + ((freq / 1000) * wait_time as u64) / 1000;

        loop {
            asm!("mrs {0}, cntpct_el0", out(reg) now);
            if now >= expire {
                break;
            }
        }
    }
}
