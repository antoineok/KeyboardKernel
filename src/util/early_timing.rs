// src/util/early_timing.rs

use core::{cell::UnsafeCell, time::Duration};

const MAX_EARLY_TIMINGS: usize = 128;

#[derive(Clone, Copy)]
pub struct EarlyTiming {
    pub label: &'static str,

    /// Absolute uptime when the operation began.
    pub started_at: Duration,

    /// Time spent inside the measured block.
    pub elapsed: Duration,
}

struct EarlyTimingBuffer {
    entries: [Option<EarlyTiming>; MAX_EARLY_TIMINGS],
    len: usize,
    dropped: usize,
}

impl EarlyTimingBuffer {
    const fn new() -> Self {
        Self {
            entries: [None; MAX_EARLY_TIMINGS],
            len: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, timing: EarlyTiming) {
        if self.len >= MAX_EARLY_TIMINGS {
            self.dropped += 1;
            return;
        }

        self.entries[self.len] = Some(timing);
        self.len += 1;
    }
}

struct GlobalEarlyTimingBuffer {
    inner: UnsafeCell<EarlyTimingBuffer>,
}

// Safe under the requirement that early timing is written from only the
// boot CPU before normal multicore execution begins.
unsafe impl Sync for GlobalEarlyTimingBuffer {}

impl GlobalEarlyTimingBuffer {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(EarlyTimingBuffer::new()),
        }
    }
}

static EARLY_TIMINGS: GlobalEarlyTimingBuffer = GlobalEarlyTimingBuffer::new();

#[inline]
pub fn record(label: &'static str, started_at: Duration, elapsed: Duration) {
    // Early boot is single-core here. Do not call this concurrently later.
    unsafe {
        (*EARLY_TIMINGS.inner.get()).push(EarlyTiming {
            label,
            started_at,
            elapsed,
        });
    }
}

pub fn flush_early_timings() {
    let buffer = unsafe { &mut *EARLY_TIMINGS.inner.get() };

    for timing in buffer.entries[..buffer.len].iter().flatten() {
        crate::info!(
            "[EARLY TIME][{}] started at {} sec {} ns; took {} sec {} ns",
            timing.label,
            timing.started_at.as_secs(),
            timing.started_at.subsec_nanos(),
            timing.elapsed.as_secs(),
            timing.elapsed.subsec_nanos(),
        );
    }

    if buffer.dropped != 0 {
        crate::info!(
            "[EARLY TIME] {} timing records were dropped",
            buffer.dropped
        );
    }

    // Drain the records so a second flush does not print them again.
    buffer.entries[..buffer.len].fill(None);
    buffer.len = 0;
    buffer.dropped = 0;
}

#[macro_export]
macro_rules! early_timeit {
    ($label:expr, $block:block) => {{
        let __timer = $crate::util::timing::TimeSpan::start();

        let __result = { $block };

        let __elapsed = __timer.elapsed();

        $crate::util::early_timing::record($label, __timer.started_at(), __elapsed);

        __result
    }};
}
