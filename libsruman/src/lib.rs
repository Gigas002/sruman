//! Steam Linux Runtime management: runtime catalog and store, installs,
//! profiles, prefixes and launch commands.
//!
//! The library never parses CLI arguments, initializes logging or reads
//! configuration files on its own; callers pass resolved values in.

/// Crate version, for state files and the User-Agent.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
