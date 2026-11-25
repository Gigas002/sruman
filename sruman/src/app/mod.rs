//! Command dispatch (4.4): one handler per command, thin over `libsruman`.
//! Handlers print through [`Printer`] and return the process exit code, or
//! an [`AppError`] whose [`AppError::exit_code`] follows lootrs: `1` runtime
//! failure, `2` invalid input or config, `3` not running, `4` busy.

use std::{io, path::Path};

use libsruman::{
    catalog::Catalog,
    launch::{LaunchContext, LaunchError},
    model::{self, Issue, LoadedGames, LoadedProfiles, Profile, Severity, ValidationContext},
    prefix::{PrefixContext, PrefixError},
    store::{Store, StoreError, import::ImportError},
};

use crate::{output::Printer, settings::Settings};

pub mod commands;
pub mod supervisor;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Bad input or configuration: an unknown name, a broken profile, a
    /// runtime that isn't installed (exit 2).
    #[error("{0}")]
    Invalid(String),

    /// Something went wrong while doing it (exit 1).
    #[error("{0}")]
    Failed(String),

    /// `stop`/`logs` on something that isn't running or has no logs (exit 3).
    #[error("{0}")]
    NotRunning(String),

    /// A game already running, a prefix or build in use, the store locked
    /// (exit 4).
    #[error("{0}")]
    Busy(String),

    /// Declined at a prompt (exit 1, as pacman).
    #[error("cancelled")]
    Cancelled,

    /// The errors were printed already; exit with this code.
    #[error("failed")]
    Reported(u8),

    /// An error reported by a detached supervisor.
    #[error("{message}")]
    Supervisor { code: u8, message: String },

    #[error("failed to write output: {0}")]
    Output(#[from] io::Error),
}

impl AppError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Invalid(_) => 2,
            Self::Failed(_) | Self::Cancelled | Self::Output(_) => 1,
            Self::NotRunning(_) => 3,
            Self::Busy(_) => 4,
            Self::Reported(code) | Self::Supervisor { code, .. } => *code,
        }
    }

    /// Stdout was closed early (`sruman runtimes | head`); not an error.
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::Output(err) if err.kind() == io::ErrorKind::BrokenPipe)
    }

    /// Whether `main` still has to print it.
    pub fn is_reported(&self) -> bool {
        matches!(self, Self::Reported(_))
    }
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::Busy(_) | StoreError::InUse { .. } => Self::Busy(err.to_string()),
            StoreError::NotInstalled { .. }
            | StoreError::BuildMissing { .. }
            | StoreError::AlreadyInstalled { .. } => Self::Invalid(err.to_string()),
            _ => Self::Failed(err.to_string()),
        }
    }
}

impl From<PrefixError> for AppError {
    fn from(err: PrefixError) -> Self {
        match err {
            PrefixError::Busy(_) => Self::Busy(err.to_string()),
            PrefixError::Io { .. } => Self::Failed(err.to_string()),
            _ => Self::Invalid(err.to_string()),
        }
    }
}

impl From<ImportError> for AppError {
    fn from(err: ImportError) -> Self {
        match err {
            ImportError::Store(err) => err.into(),
            err => Self::Invalid(err.to_string()),
        }
    }
}

#[cfg(feature = "download")]
impl From<libsruman::install::InstallError> for AppError {
    fn from(err: libsruman::install::InstallError) -> Self {
        use libsruman::install::InstallError;
        match err {
            InstallError::Store(err) => err.into(),
            InstallError::Cancelled => Self::Cancelled,
            InstallError::Catalog(_)
            | InstallError::NotSynced(_)
            | InstallError::NoChannelBuild { .. }
            | InstallError::NoArchive { .. } => Self::Invalid(err.to_string()),
            err => Self::Failed(err.to_string()),
        }
    }
}

#[cfg(feature = "download")]
impl From<libsruman::sync::SyncError> for AppError {
    fn from(err: libsruman::sync::SyncError) -> Self {
        match err {
            libsruman::sync::SyncError::UnknownRuntime(_) => Self::Invalid(err.to_string()),
            err => Self::Failed(err.to_string()),
        }
    }
}

/// `prepare`'s problems: a missing executable, runtime or wrapper (exit 2).
impl From<LaunchError> for AppError {
    fn from(err: LaunchError) -> Self {
        Self::Invalid(format!("not launching:\n{}", indent(&err.problems)))
    }
}

/// An issue without its `error: `/`warning: ` prefix, for printers that
/// add their own.
pub fn issue_text(issue: &Issue) -> String {
    let text = issue.to_string();
    ["error: ", "warning: "]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .map_or_else(|| text.clone(), str::to_owned)
}

/// Lines indented by two spaces.
pub fn indent(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What every handler gets.
pub struct App<'a> {
    pub settings: &'a Settings,
    pub out: &'a mut Printer,
}

impl App<'_> {
    pub fn store(&self) -> Store {
        Store::open(&self.settings.dirs.app.runtimes)
    }

    pub fn catalog(&self) -> &Catalog {
        &self.settings.catalog
    }

    pub fn home(&self) -> &Path {
        &self.settings.dirs.home
    }

    #[cfg(feature = "download")]
    pub fn sync_dir(&self) -> libsruman::sync::SyncDir {
        libsruman::sync::SyncDir::new(&self.settings.dirs.app.sync_cache)
    }

    pub fn profiles(&self) -> LoadedProfiles {
        model::load_dir(&self.settings.dirs.app.profiles)
    }

    pub fn games(&self) -> LoadedGames {
        model::load_games(&self.settings.dirs.app.games_file)
    }

    pub fn validation(&self) -> ValidationContext<'_> {
        ValidationContext {
            home: self.home(),
            catalog: self.catalog(),
        }
    }

    pub fn prefix_ctx(&self) -> PrefixContext<'_> {
        PrefixContext {
            lock_dir: &self.settings.dirs.app.locks,
            home: self.home(),
        }
    }

    /// The context for `prepare`/`preflight`, borrowing `store`.
    pub fn launch_ctx<'s>(
        &'s self,
        store: &'s Store,
        debug: bool,
        extra_args: &'s [String],
    ) -> LaunchContext<'s> {
        LaunchContext {
            home: self.home(),
            search_path: &self.settings.search_path,
            debug,
            extra_args,
            store,
            catalog: self.catalog(),
            shell: self.settings.shell.as_deref(),
            host_libs: None,
        }
    }

    /// The profile `name` (a file or an implicit one), or exit 2.
    pub fn profile(&self, profiles: &LoadedProfiles, name: &str) -> Result<Profile, AppError> {
        profiles.get(name, self.catalog()).ok_or_else(|| {
            AppError::Invalid(format!(
                "no profile named `{name}`; create it with `sruman profiles new {name} --runtime <runtime>`"
            ))
        })
    }

    /// Prints warnings; fails with the errors.
    pub fn check_issues(&mut self, issues: &[Issue]) -> Result<(), AppError> {
        let mut errors = Vec::new();
        for issue in issues {
            match issue.severity {
                Severity::Warning => {
                    tracing::warn!("{issue}");
                    self.out.warning(issue_text(issue))?;
                }
                Severity::Error => errors.push(issue_text(issue)),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Invalid(errors.join("\n")))
        }
    }
}

/// Runs the command in `settings`; returns the exit code.
pub fn run(settings: &Settings, out: &mut Printer) -> Result<u8, AppError> {
    let mut app = App { settings, out };
    tracing::info!(command = settings.command.name(), "start");
    if settings.stale_steam_library {
        app.out.warning(
            "config.toml: `steam_library` does nothing now: moving Steam's folders is \
             `gaki-cli steam relocate`, which uses gaki's `steam_library`; remove the key",
        )?;
    }
    commands::dispatch(&mut app, &settings.command)
}
