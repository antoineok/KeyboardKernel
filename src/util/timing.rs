use crate::arch::time::uptime;
use core::time::Duration;

pub struct TimeSpan {
    start: Duration,
}

impl TimeSpan {
    #[inline(always)]
    pub fn start() -> Self {
        TimeSpan { start: uptime() }
    }

    #[inline(always)]
    pub fn elapsed(&self) -> Duration {
        let now = uptime();
        now.checked_sub(self.start).unwrap_or(Duration::ZERO)
    }
}

#[macro_export]
macro_rules! timeit {
    ($label:expr, $block:block) => {{
        let t = $crate::util::timing::TimeSpan::start();
        let result = { $block };
        let dur = t.elapsed();
        crate::info!(
            "[TIME][{}] {:?} sec {:?} ns",
            $label,
            dur.as_secs(),
            dur.subsec_nanos()
        );
        result
    }};
}
