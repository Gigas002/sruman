//! Steam Linux Runtime management: runtime catalog and store, installs,
//! profiles, prefixes and launch commands.
//!
//! The library never parses CLI arguments, initializes logging or reads
//! configuration files on its own; callers pass resolved values in.

pub mod catalog;
#[cfg(feature = "download")]
pub mod fetch;
pub mod manifest;
pub mod model;
pub mod mtree;
pub mod paths;
pub mod prefix;
pub mod progress;
pub mod store;
#[cfg(feature = "download")]
pub mod sync;
pub mod version;

/// Crate version, for state files and the User-Agent.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
