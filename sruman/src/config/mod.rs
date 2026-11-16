//! `config.toml` discovery, reading and deserialization.
//!
//! Every key is optional; precedence and defaults are applied in
//! `settings`. Unknown keys are an error, so a typo never silently does
//! nothing.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use libsruman::{catalog::RuntimeOverride, version::Channel};
use serde::Deserialize;

#[cfg(test)]
mod tests;

pub const FILE_NAME: &str = "config.toml";

/// Raw contents of `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub log_level: Option<LogLevel>,
    /// Daily app logs and run logs per game/profile to keep.
    pub log_keep: Option<usize>,
    /// Profile for path runs and shells without `-p`.
    pub default_profile: Option<String>,
    pub channel: Option<Channel>,
    /// Per-runtime channels, `[channels] sniper = "beta"`.
    #[serde(default)]
    pub channels: BTreeMap<String, Channel>,
    /// Installed builds kept per runtime by `-Sc`.
    pub keep_versions: Option<usize>,
    /// Install a missing runtime at launch without asking (D12).
    pub auto_install: Option<bool>,
    pub color: Option<ColorChoice>,
    pub prefix_root: Option<PathBuf>,
    /// SIGTERM → SIGKILL grace period for `-Xk`.
    pub stop_timeout_secs: Option<u64>,
    /// `[[runtime]]` catalog extensions and overrides (D8).
    #[serde(default)]
    pub runtime: Vec<RuntimeOverride>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// `--color` / `color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    /// Colors when stdout is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },

    #[error("invalid {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Loads `path`. A missing file is `Ok(None)` unless `required` (an
/// explicit `--config`).
pub fn load(path: &Path, required: bool) -> Result<Option<FileConfig>, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound && !required => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };

    parse(&content)
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
}

/// Parses `config.toml` contents.
pub fn parse(content: &str) -> Result<FileConfig, toml::de::Error> {
    toml::from_str(content)
}
