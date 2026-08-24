#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        {
            if std::env::var("DEBUG_LOG").is_ok() {
                println!("[DEBUG] [{}:{}] {}", file!(), line!(), format_args!($($arg)*));
            }
        }
    };
}
