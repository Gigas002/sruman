//! Games and profiles as stored in `games.toml` and `profiles/*.toml`,
//! plus validation.
//!
//! Profiles are in the libgameprofiles format, shared with lootrs and gaki: a
//! sruman profile is a `native` profile with an `[slr]` section
//! ([`SlrSection`]). Games follow lootrs' `games.toml`.
//! Validation never fails fast: it returns every [`Issue`] it finds, so
//! `check` shows them all at once.

use std::path::Path;

mod game;
mod profile;

pub use game::{Game, LoadedGames, RunTarget, TargetError, load_games, resolve_target};
pub use libgameprofiles::{Issue, Issues, Severity, Subject, validate_name};
pub use profile::{
    LibOverride, LoadedProfiles, Profile, Runner, ShaderCache, SlrMode, SlrSection,
    implicit_profile, load_dir, new_profile, save_profile, slr, validate_profile,
};

use crate::catalog::Catalog;

#[cfg(test)]
mod tests;

/// Values validation depends on.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext<'a> {
    /// Used to expand `~` in paths.
    pub home: &'a Path,
    /// The runtime catalog, to check `[slr] runtime` names.
    pub catalog: &'a Catalog,
}

/// Warns about keys a file has that the schema doesn't know, instead of
/// rejecting the file. `known` lists dotted paths (`slr.runtime`); a table
/// key ending in `.*` accepts anything below it (`env.*`).
pub(crate) fn unknown_keys(value: &toml::Value, known: &[&str], issues: &mut Issues) {
    fn walk(value: &toml::Value, prefix: &str, known: &[&str], issues: &mut Issues) {
        let toml::Value::Table(table) = value else {
            return;
        };
        for (key, child) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if known.contains(&format!("{path}.*").as_str()) {
                continue;
            }
            if !known.contains(&path.as_str()) {
                issues.warn(format!("unknown key `{path}` is ignored"));
            } else if child.is_table() {
                walk(child, &path, known, issues);
            }
        }
    }
    walk(value, "", known, issues);
}

/// Errors from loading or saving profiles.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// Reading or writing a profile file failed.
    #[error(transparent)]
    Store(libgameprofiles::store::Error),
    /// A profile name that isn't allowed (the reason).
    #[error("invalid profile name: {0}")]
    InvalidName(String),
}

impl From<libgameprofiles::store::Error> for ModelError {
    fn from(err: libgameprofiles::store::Error) -> Self {
        match err {
            libgameprofiles::store::Error::InvalidName(message) => Self::InvalidName(message),
            err => Self::Store(err),
        }
    }
}
