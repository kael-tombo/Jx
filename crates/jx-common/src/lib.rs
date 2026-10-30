//! Shared infrastructure for jx: diagnosed errors, colour handling,
//! `JX_HOME`/settings resolution and the content-addressed artifact cache.
//!
//! This crate exists so that every other crate reports failures the same way
//! (error::JxError) and shares one notion of where jx keeps state (paths,
//! settings, cache). It is intentionally dependency-light.

pub mod cache;
pub mod color;
pub mod config;
pub mod error;
pub mod paths;

/// How much free disk space (bytes) jx considers minimal for a JDK download.
/// A heuristic; surfaced by `jx doctor` as a warning, never a hard error.
pub const MIN_FREE_DISK_BYTES: u64 = 1_000_000_000;
