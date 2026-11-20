//! Pure parsers for everything zfscope reads. No I/O here; each is unit-tested against captured text.

pub mod disk;
pub mod history;
pub mod iostat;
pub mod kstat;
pub mod zjson;
