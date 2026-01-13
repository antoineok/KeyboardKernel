#[macro_export]
macro_rules! log {
    ($level:expr, $($arg:tt)*) => {
        crate::uart_println!("{:#?} - {}: {}", crate::time::time_manager().uptime().as_millis(), $level, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        crate::log!("INFO", "{}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        crate::log!("WARN", "{}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        crate::log!("ERROR", "{}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        crate::log!("TRACE", "{}", format_args!($($arg)*))
    };
}

#[cfg(feature = "jtag")]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        crate::log!("DEBUG", "{}", format_args!($($arg)*))
    };
}
