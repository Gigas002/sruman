//! Unified resolver: CLI > `config.toml` > built-in defaults.
//!
//! [`Settings`] is the only configuration type passed below `main`.

use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, time::Duration};

use libsruman::{
    catalog::{Catalog, CatalogError},
    model,
    paths::{self, AppDirs, PathsError, XdgDirs},
    version::Channel,
};

use crate::config::{self, ColorChoice, ConfigError, LogLevel};

pub mod command;

pub use command::{Command, UsageError};

#[cfg(test)]
mod tests;

pub const DEFAULT_LOG_KEEP: usize = 10;
pub const DEFAULT_KEEP_VERSIONS: usize = 2;
pub const DEFAULT_STOP_TIMEOUT_SECS: u64 = 5;

/// Values taken from the command line, already converted from `clap` types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOptions {
    pub command: Command,
    /// `--json`, on the commands that have it.
    pub json: bool,
    /// `-q`.
    pub quiet: bool,
    /// `--config <file>`.
    pub config: Option<PathBuf>,
    pub color: Option<ColorChoice>,
    /// `-y`/`--yes`.
    pub yes: bool,
    /// `-v` count.
    pub verbose: u8,
}

impl CliOptions {
    /// `command` with every global option at its default.
    pub fn new(command: Command) -> Self {
        Self {
            command,
            json: false,
            quiet: false,
            config: None,
            color: None,
            yes: false,
            verbose: 0,
        }
    }
}

/// The process environment that settings depend on.
#[derive(Debug, Clone)]
pub struct Environment<F> {
    /// Usually `std::env::var_os`.
    pub lookup: F,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub command: Command,
    pub dirs: Dirs,
    /// The built-in catalog with `[[runtime]]` applied.
    pub catalog: Catalog,
    pub channels: Channels,
    /// Installed builds kept per runtime by `clean`.
    pub keep_versions: usize,
    /// Install a missing runtime at launch without asking (D12).
    pub auto_install: bool,
    /// For `execute` and `shell` without `-p`.
    pub default_profile: Option<String>,
    /// SIGTERM → SIGKILL grace period for `stop`.
    pub stop_timeout: Duration,
    pub output: Output,
    pub logging: Logging,
    /// Absolute directories from `PATH`, for wrappers.
    pub search_path: Vec<PathBuf>,
    /// `$SHELL`, for shells in overlay mode.
    pub shell: Option<PathBuf>,
    /// `$VISUAL` or `$EDITOR` (may include arguments), for `profiles edit`.
    pub editor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub home: PathBuf,
    /// The working directory, for run targets.
    pub cwd: PathBuf,
    /// The `config.toml` in use (it may not exist).
    pub config_file: PathBuf,
    /// XDG defaults: profiles, games file, store, caches, logs, locks.
    pub app: AppDirs,
    /// Where new prefixes go by default.
    pub prefix_root: PathBuf,
    /// `$XDG_DATA_HOME`, where Steam lives unless `~/.steam/root` says.
    pub data_home: PathBuf,
    /// `steam_dir` from `config.toml`, absolute.
    pub steam_dir: Option<PathBuf>,
    /// `steam_library` from `config.toml`, absolute.
    pub steam_library: Option<PathBuf>,
    /// `$XDG_CACHE_HOME` and `$XDG_STATE_HOME`, where `steam relocate`
    /// puts Steam's caches and logs.
    pub cache_home: PathBuf,
    pub state_home: PathBuf,
}

/// The channel of each runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Channels {
    pub default: Channel,
    pub per_runtime: BTreeMap<String, Channel>,
}

impl Channels {
    pub fn get(&self, runtime: &str) -> Channel {
        self.per_runtime
            .get(runtime)
            .copied()
            .unwrap_or(self.default)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    /// `auto` is decided against the terminal when printing.
    pub color: ColorChoice,
    pub json: bool,
    pub quiet: bool,
    /// Answer every `[Y/n]` with its default.
    pub yes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logging {
    /// The daily app log's level.
    pub level: LogLevel,
    /// Mirror the log to stderr at this level (`-v` debug, `-vv` trace).
    pub stderr: Option<LogLevel>,
    pub dir: PathBuf,
    /// Daily app logs, and run logs per game/profile, to keep.
    pub keep: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Usage(#[from] UsageError),

    #[error(transparent)]
    Paths(#[from] PathsError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("{}: {}", path.display(), join_errors(.errors))]
    Catalog {
        path: PathBuf,
        errors: Vec<CatalogError>,
    },

    #[error("{}: `{key}`: {message}", path.display())]
    Invalid {
        path: PathBuf,
        key: &'static str,
        message: String,
    },
}

fn join_errors(errors: &[CatalogError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

impl Settings {
    pub fn resolve<F>(cli: CliOptions, env: Environment<F>) -> Result<Self, SettingsError>
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let lookup = &env.lookup;
        let xdg = XdgDirs::from_lookup(lookup)?;
        let app = AppDirs::new(&xdg);

        let (config_file, required) = match &cli.config {
            Some(path) => (env.cwd.join(path), true),
            None => (app.config.join(config::FILE_NAME), false),
        };
        let file = config::load(&config_file, required)?.unwrap_or_default();
        let invalid = |key, message: String| SettingsError::Invalid {
            path: config_file.clone(),
            key,
            message,
        };

        if let Some(profile) = &file.default_profile {
            model::validate_name(profile).map_err(|err| invalid("default_profile", err))?;
        }
        for name in file.channels.keys() {
            libsruman::version::validate_runtime_name(name)
                .map_err(|err| invalid("channels", err.to_string()))?;
        }
        let keep = file.log_keep.unwrap_or(DEFAULT_LOG_KEEP);
        if keep == 0 {
            return Err(invalid("log_keep", "must be at least 1".into()));
        }
        let prefix_root = match file.prefix_root.as_deref() {
            None => app.prefixes.clone(),
            Some(path) => {
                let expanded = paths::expand_tilde(path, &xdg.home);
                if !expanded.is_absolute() {
                    return Err(invalid(
                        "prefix_root",
                        format!("must be absolute or start with `~`, got {}", path.display()),
                    ));
                }
                expanded
            }
        };
        let absolute = |key, path: Option<&std::path::Path>| match path {
            None => Ok(None),
            Some(path) => {
                let expanded = paths::expand_tilde(path, &xdg.home);
                if expanded.is_absolute() {
                    Ok(Some(expanded))
                } else {
                    Err(invalid(
                        key,
                        format!("must be absolute or start with `~`, got {}", path.display()),
                    ))
                }
            }
        };
        let steam_dir = absolute("steam_dir", file.steam_dir.as_deref())?;
        let steam_library = absolute("steam_library", file.steam_library.as_deref())?;
        let catalog = Catalog::builtin()
            .with_overrides(&file.runtime)
            .map_err(|errors| SettingsError::Catalog {
                path: config_file.clone(),
                errors,
            })?;

        let ctx = command::Context {
            cwd: &env.cwd,
            home: &xdg.home,
            default_profile: file.default_profile.as_deref(),
        };
        let command = cli.command.resolve(&ctx)?;

        let stderr = match cli.verbose {
            0 => None,
            1 => Some(LogLevel::Debug),
            _ => Some(LogLevel::Trace),
        };
        let file_level = file.log_level.unwrap_or_default();

        let color = match cli.color.or(file.color).unwrap_or_default() {
            ColorChoice::Auto if lookup("NO_COLOR").is_some_and(|v| !v.is_empty()) => {
                ColorChoice::Never
            }
            color => color,
        };
        let search_path = lookup("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .filter(|dir| dir.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        let shell = lookup("SHELL")
            .map(PathBuf::from)
            .filter(|shell| !shell.as_os_str().is_empty());
        let editor = ["VISUAL", "EDITOR"].into_iter().find_map(|var| {
            lookup(var)
                .map(|value| value.to_string_lossy().trim().to_owned())
                .filter(|value| !value.is_empty())
        });

        Ok(Self {
            command,
            catalog,
            channels: Channels {
                default: file.channel.unwrap_or_default(),
                per_runtime: file.channels,
            },
            keep_versions: file.keep_versions.unwrap_or(DEFAULT_KEEP_VERSIONS),
            auto_install: file.auto_install.unwrap_or(false),
            default_profile: file.default_profile,
            stop_timeout: Duration::from_secs(
                file.stop_timeout_secs.unwrap_or(DEFAULT_STOP_TIMEOUT_SECS),
            ),
            output: Output {
                color,
                json: cli.json,
                quiet: cli.quiet,
                yes: cli.yes,
            },
            logging: Logging {
                level: stderr.map_or(file_level, |level| level.max(file_level)),
                stderr,
                dir: app.logs.clone(),
                keep,
            },
            search_path,
            shell,
            editor,
            dirs: Dirs {
                home: xdg.home,
                cwd: env.cwd,
                config_file,
                app,
                prefix_root,
                data_home: xdg.data_home,
                steam_dir,
                steam_library,
                cache_home: xdg.cache_home,
                state_home: xdg.state_home,
            },
        })
    }
}
