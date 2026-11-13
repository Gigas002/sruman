//! XDG Base Directory resolution for sruman.
//!
//! Everything here is pure: environment values come in through a lookup
//! function, so callers decide where they are read from. lootrs and gaki use
//! [`AppDirs`] to find the shared runtime store.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

/// Directory name used under every XDG base directory.
pub const APP_NAME: &str = "sruman";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    #[error("HOME is not set or is not an absolute path")]
    NoHome,
}

/// XDG base directories, with the spec's fallbacks applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdgDirs {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, fallback `~/.config`.
    pub config_home: PathBuf,
    /// `$XDG_DATA_HOME`, fallback `~/.local/share`.
    pub data_home: PathBuf,
    /// `$XDG_STATE_HOME`, fallback `~/.local/state`.
    pub state_home: PathBuf,
    /// `$XDG_CACHE_HOME`, fallback `~/.cache`.
    pub cache_home: PathBuf,
    /// `$XDG_RUNTIME_DIR`; the spec defines no fallback.
    pub runtime_dir: Option<PathBuf>,
}

impl XdgDirs {
    /// Resolves base directories from an environment lookup, e.g. `std::env::var_os`.
    ///
    /// Per the spec, empty or relative values are ignored and the fallback is used.
    pub fn from_lookup<F>(lookup: F) -> Result<Self, PathsError>
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let home = absolute_var(&lookup, "HOME").ok_or(PathsError::NoHome)?;
        let base = |var, fallback: &str| absolute_var(&lookup, var).unwrap_or(home.join(fallback));

        Ok(Self {
            config_home: base("XDG_CONFIG_HOME", ".config"),
            data_home: base("XDG_DATA_HOME", ".local/share"),
            state_home: base("XDG_STATE_HOME", ".local/state"),
            cache_home: base("XDG_CACHE_HOME", ".cache"),
            runtime_dir: absolute_var(&lookup, "XDG_RUNTIME_DIR"),
            home,
        })
    }
}

/// Default sruman directories derived from [`XdgDirs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// `config.toml` and `games.toml`.
    pub config: PathBuf,
    /// `profiles/<name>.toml`.
    pub profiles: PathBuf,
    /// The games list.
    pub games_file: PathBuf,
    /// Installed runtime builds, `<suite>/<build_id>/`. Shared by every
    /// client of the library.
    pub runtimes: PathBuf,
    /// Suggested root for new prefixes; a prefix may live anywhere.
    pub prefixes: PathBuf,
    /// The `-Sy` sync index.
    pub sync_cache: PathBuf,
    /// Downloaded archives.
    pub pkg_cache: PathBuf,
    /// App log and per-run logs.
    pub logs: PathBuf,
    /// Lock files; falls back to the state directory without `$XDG_RUNTIME_DIR`.
    pub locks: PathBuf,
}

impl AppDirs {
    pub fn new(xdg: &XdgDirs) -> Self {
        let config = xdg.config_home.join(APP_NAME);
        let data = xdg.data_home.join(APP_NAME);
        let state = xdg.state_home.join(APP_NAME);
        let cache = xdg.cache_home.join(APP_NAME);

        Self {
            profiles: config.join("profiles"),
            games_file: config.join("games.toml"),
            config,
            runtimes: data.join("runtimes"),
            prefixes: data.join("prefixes"),
            sync_cache: cache.join("sync"),
            pkg_cache: cache.join("pkg"),
            logs: state.join("logs"),
            locks: xdg
                .runtime_dir
                .as_ref()
                .map_or_else(|| state.join("run"), |dir| dir.join(APP_NAME)),
        }
    }
}

/// Expands a leading `~` (alone or followed by `/`) to `home`.
///
/// `~user` forms are left untouched.
pub fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn absolute_var<F>(lookup: &F, var: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    lookup(var)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}
