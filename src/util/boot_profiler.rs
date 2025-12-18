use crate::arch::time::uptime;
use crate::info;
use core::fmt::{self, Write};
use core::time::Duration;

// ---------------------------------------------
// Boot event types
// ---------------------------------------------

pub struct BootEvent {
    pub label: &'static str,
    pub formatted: Option<&'static str>, // stored formatted text
    pub time: Duration,
}

static mut EVENTS: [Option<BootEvent>; 2048] = [const { None }; 2048];
static mut INDEX: usize = 0;

// ---------------------------------------------
// Small static buffer arena for formatted strings
// ---------------------------------------------

static mut FORMAT_BUF: [u8; 4096] = [0; 4096];
static mut FORMAT_OFF: usize = 0;

/// Extremely small write adapter using our static buffer.
/// Returns a &'static str.
fn store_formatted(s: &[u8]) -> Option<&'static str> {
    unsafe {
        if FORMAT_OFF + s.len() >= FORMAT_BUF.len() {
            return None; // buffer full
        }

        let start = FORMAT_OFF;
        FORMAT_BUF[FORMAT_OFF..FORMAT_OFF + s.len()].copy_from_slice(s);
        FORMAT_OFF += s.len();

        Some(core::str::from_utf8_unchecked(
            &FORMAT_BUF[start..start + s.len()],
        ))
    }
}

// A tiny formatter for use before heap + UART
struct ScratchWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Write for ScratchWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.pos + bytes.len() > self.buf.len() {
            return Err(fmt::Error);
        }
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }
}

impl<'a> ScratchWriter<'a> {
    fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.pos]) }
    }
}

// ---------------------------------------------
// Public API
// ---------------------------------------------

/// Simple boot mark without formatting.
pub fn boot_mark(label: &'static str) {
    unsafe {
        if INDEX < EVENTS.len() {
            EVENTS[INDEX] = Some(BootEvent {
                label,
                formatted: None,
                time: uptime(),
            });
            INDEX += 1;
        }
    }
}

/// Boot mark with format!-like formatting (no heap).
#[macro_export]
macro_rules! boot_mark {
    ($($arg:tt)*) => {
        $crate::util::boot_profiler::boot_mark_fmt(format_args!($($arg)*))
    };
}

pub fn boot_mark_fmt(args: fmt::Arguments<'_>) {
    let mut temp = [0u8; 1024];
    let mut w = ScratchWriter {
        buf: &mut temp,
        pos: 0,
    };

    if write!(&mut w, "{}", args).is_err() {
        return;
    }

    let len = w.pos;
    drop(w); // release mutable borrow

    if let Some(formatted) = store_formatted(&temp[..len]) {
        unsafe {
            if INDEX < EVENTS.len() {
                EVENTS[INDEX] = Some(BootEvent {
                    label: "",
                    formatted: Some(formatted),
                    time: uptime(),
                });
                INDEX += 1;
            }
        }
    }
}

/// Print all boot events after UART/logger are initialized.
pub fn boot_report() {
    unsafe {
        for ev in EVENTS.iter().flatten() {
            let label = if let Some(fmt) = ev.formatted {
                fmt
            } else {
                ev.label
            };

            info!(
                "[BOOT] {} at {}s {}ns",
                label,
                ev.time.as_secs(),
                ev.time.subsec_nanos()
            );
        }
    }
}
