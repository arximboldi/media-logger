//! media-logger — a tamper-evident log of media playback, output device and
//! volume.
//!
//! The binary is a thin shell over these modules. They are a library so that
//! anything else needing to write a log — the example generator, a future
//! importer — goes through the same writer rather than reimplementing the
//! hash chain, which is the one piece that must never exist twice.

pub mod anchor;
pub mod audio;
pub mod event;
pub mod log;
pub mod mpris;
pub mod query;
pub mod quiet;
pub mod render;
