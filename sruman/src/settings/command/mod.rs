//! What an invocation does: one [`Command`] per row of the CLI tables in
//! `docs/PLAN.md`. `cli` builds it from clap's parse (which already checked
//! options, runtime refs and names); [`Command::resolve`] fills in what
//! needs the environment: paths against the working directory, and
//! `default_profile` for `execute` and `shell` without `-p`.

use std::path::{Path, PathBuf};

use libsruman::{
    model::{self, SlrMode},
    paths,
    version::RuntimeRef,
};

#[cfg(test)]
mod tests;

/// One command, with its own options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `sync [--force]`.
    #[cfg(feature = "download")]
    Sync { force: bool },
    /// `install <rt>[=build]…`.
    #[cfg(feature = "download")]
    Install {
        targets: Vec<RuntimeRef>,
        needed: bool,
        download_only: bool,
    },
    /// `upgrade [rt…]`; empty upgrades every installed runtime.
    #[cfg(feature = "download")]
    Upgrade {
        targets: Vec<String>,
        /// `--sync`: refresh the index first.
        sync: bool,
        allow_downgrade: bool,
        download_only: bool,
    },
    /// `outdated [rt…]`.
    #[cfg(feature = "download")]
    Outdated { targets: Vec<String> },
    /// `remove <rt>[=build]…`.
    Remove {
        targets: Vec<RuntimeRef>,
        /// Skip the profile check (a build in use is never removed).
        force: bool,
    },
    /// `search [regex…]`.
    #[cfg(feature = "download")]
    Search {
        terms: Vec<String>,
        /// `--packages`: search the package manifests.
        packages: bool,
    },
    /// `info <rt>[=build]…`: installed builds, else the sync index.
    Info {
        targets: Vec<RuntimeRef>,
        packages: bool,
    },
    /// `runtimes`: installed runtimes (`-q`: names only).
    Runtimes { quiet: bool },
    /// `builds <rt>`: builds on the server.
    #[cfg(feature = "download")]
    Builds { runtime: String },
    /// `files <rt>[=build]`.
    Files { runtime: RuntimeRef },
    /// `verify [rt[=build]…]`; empty checks every installed build.
    Verify {
        targets: Vec<RuntimeRef>,
        /// `--full`: content hashes.
        full: bool,
    },
    /// `clean [--all]`.
    #[cfg(feature = "download")]
    Clean { all: bool },
    /// `import <archive|dir>…`; absolute after [`Command::resolve`].
    Import { sources: Vec<PathBuf>, link: bool },

    /// `run <game> [-- args…]`.
    Run(Run),
    /// `execute -p <profile> <path|program> [-- args…]`.
    Execute(Execute),
    /// `shell [-p <profile>]`; the profile is set after
    /// [`Command::resolve`].
    Shell {
        profile: Option<String>,
        debug: bool,
    },
    /// `status`.
    Status,
    /// `stop <game|profile|pid>…`.
    Stop { targets: Vec<String> },
    /// `killall`.
    Killall,
    /// `list`: games.
    List,
    /// `show <game>`.
    Show { game: String },
    /// `check [game|profile…]`; empty checks everything.
    Check { targets: Vec<String> },
    /// `logs <game> [--path]`.
    Logs { game: String, path_only: bool },
    /// `init <profile>`.
    Init { profile: String },
    /// `profiles …`.
    Profiles(Profiles),
    /// `doctor [rt…]`, for the named runtimes or every installed one.
    #[cfg(feature = "diag")]
    Doctor { runtimes: Vec<RuntimeRef> },
}

/// `run <game>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub game: String,
    pub args: Vec<String>,
    /// `-p`: another profile for this run.
    pub profile: Option<String>,
    pub wait: bool,
    pub debug: bool,
}

/// `execute <path|program>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execute {
    /// A path (absolute after [`Command::resolve`]), or a bare program name
    /// looked up in the runtime's `PATH`.
    pub target: PathBuf,
    pub args: Vec<String>,
    /// `-p`, else `default_profile` (required after [`Command::resolve`]).
    pub profile: Option<String>,
    pub wait: bool,
    pub debug: bool,
}

/// `profiles …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profiles {
    List,
    Show {
        name: String,
    },
    New {
        name: String,
        runtime: RuntimeRef,
        mode: SlrMode,
        /// Absolute after [`Command::resolve`]; `None` uses
        /// `prefix_root/<name>`.
        prefix: Option<PathBuf>,
    },
    Edit {
        name: String,
    },
    EditGames,
    Delete {
        names: Vec<String>,
        /// Also delete the prefix, unless shared or in use.
        prefix: bool,
    },
}

/// An invocation that's valid to clap but not here (exit 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

/// What resolution needs from outside the command line.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// `default_profile` from `config.toml`.
    pub default_profile: Option<&'a str>,
}

impl Command {
    /// The command as typed, e.g. `profiles new`, for messages.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "download")]
            Self::Sync { .. } => "sync",
            #[cfg(feature = "download")]
            Self::Install { .. } => "install",
            #[cfg(feature = "download")]
            Self::Upgrade { .. } => "upgrade",
            #[cfg(feature = "download")]
            Self::Outdated { .. } => "outdated",
            Self::Remove { .. } => "remove",
            #[cfg(feature = "download")]
            Self::Search { .. } => "search",
            Self::Info { .. } => "info",
            Self::Runtimes { .. } => "runtimes",
            #[cfg(feature = "download")]
            Self::Builds { .. } => "builds",
            Self::Files { .. } => "files",
            Self::Verify { .. } => "verify",
            #[cfg(feature = "download")]
            Self::Clean { .. } => "clean",
            Self::Import { .. } => "import",
            Self::Run(_) => "run",
            Self::Execute(_) => "execute",
            Self::Shell { .. } => "shell",
            Self::Status => "status",
            Self::Stop { .. } => "stop",
            Self::Killall => "killall",
            Self::List => "list",
            Self::Show { .. } => "show",
            Self::Check { .. } => "check",
            Self::Logs { .. } => "logs",
            Self::Init { .. } => "init",
            Self::Profiles(profiles) => match profiles {
                Profiles::List => "profiles list",
                Profiles::Show { .. } => "profiles show",
                Profiles::New { .. } => "profiles new",
                Profiles::Edit { .. } => "profiles edit",
                Profiles::EditGames => "profiles edit-games",
                Profiles::Delete { .. } => "profiles delete",
            },
            #[cfg(feature = "diag")]
            Self::Doctor { .. } => "doctor",
        }
    }

    /// Whether this starts a game, a program or a shell.
    pub fn is_launch(&self) -> bool {
        matches!(self, Self::Run(_) | Self::Execute(_) | Self::Shell { .. })
    }

    /// Makes paths absolute and fills in `default_profile`.
    pub fn resolve(self, ctx: &Context) -> Result<Self, UsageError> {
        Ok(match self {
            Self::Import { sources, link } => Self::Import {
                sources: sources.iter().map(|s| cli_path(s, ctx)).collect(),
                link,
            },
            Self::Execute(mut execute) => {
                if is_path(&execute.target) {
                    execute.target = cli_path(&execute.target, ctx);
                }
                execute.profile = Some(profile_or_default(execute.profile, ctx, "execute")?);
                Self::Execute(execute)
            }
            Self::Shell { profile, debug } => Self::Shell {
                profile: Some(profile_or_default(profile, ctx, "shell")?),
                debug,
            },
            Self::Profiles(Profiles::New {
                name,
                runtime,
                mode,
                prefix,
            }) => Self::Profiles(Profiles::New {
                name,
                runtime,
                mode,
                prefix: prefix.map(|p| cli_path(&p, ctx)),
            }),
            other => other,
        })
    }
}

/// `-p`, else `default_profile`, else an error naming both.
fn profile_or_default(
    profile: Option<String>,
    ctx: &Context,
    command: &str,
) -> Result<String, UsageError> {
    match (profile, ctx.default_profile) {
        (Some(profile), _) => Ok(profile),
        (None, Some(profile)) => {
            model::validate_name(profile).map_err(UsageError)?;
            Ok(profile.to_owned())
        }
        (None, None) => Err(UsageError(format!(
            "{command} needs a profile: -p <profile>, or default_profile in config.toml"
        ))),
    }
}

/// A path rather than a program name: it has a `/` or starts with `~`.
fn is_path(target: &Path) -> bool {
    target.components().count() > 1 || target.is_absolute() || target.starts_with("~")
}

/// A path from the command line: `~` expanded (for quoted ones), relative
/// to the working directory.
fn cli_path(path: &Path, ctx: &Context) -> PathBuf {
    ctx.cwd.join(paths::expand_tilde(path, ctx.home))
}
