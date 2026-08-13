pub mod config;
pub mod core;
pub mod platform;

#[cfg(target_os = "linux")]
pub use platform::linux as platform_current;
#[cfg(windows)]
pub use platform::windows as platform_current;
