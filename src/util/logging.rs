

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        crate::uart_println!("INFO: {}", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        crate::uart_println!("WARN: {}", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        crate::uart_println!("ERROR: {}", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        crate::uart_println!("DEBUG: {}", format_args!($($arg)*));
    };
}