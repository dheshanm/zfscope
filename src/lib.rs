//! zfscope: a live, read-only TUI for OpenZFS hosts.

/// `0.1.0+b12` (release build number, see build.rs).
pub const VERSION: &str = env!("ZFSCOPE_VERSION");

pub mod app;
pub mod collect;
pub mod dump;
pub mod fmt;
pub mod help;
pub mod model;
pub mod parse;
pub mod replay;
pub mod source;
pub mod ui;
pub mod workers;
