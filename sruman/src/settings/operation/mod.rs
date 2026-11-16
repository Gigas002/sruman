//! Turns pacman's flag soup (one operation, modifiers scoped to it, then
//! targets) into a typed [`Operation`], rejecting modifiers that don't
//! belong to the chosen operation with pacman-like messages.

use std::path::{Path, PathBuf};

use libsruman::{model, paths, version::RuntimeRef};

#[cfg(test)]
mod tests;

/// An operation flag as given on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpFlag {
    #[cfg(feature = "download")]
    Sync,
    Query,
    Remove,
    Upgrade,
    Exec,
    Games,
    Profiles,
    #[cfg(feature = "diag")]
    Doctor,
}

impl OpFlag {
    pub fn spelling(self) -> &'static str {
        match self {
            #[cfg(feature = "download")]
            Self::Sync => "-S",
            Self::Query => "-Q",
            Self::Remove => "-R",
            Self::Upgrade => "-U",
            Self::Exec => "-X",
            Self::Games => "-G",
            Self::Profiles => "-P",
            #[cfg(feature = "diag")]
            Self::Doctor => "-D",
        }
    }
}

/// Lowercase modifiers and per-operation long options, as given. A letter
/// means different things under different operations, as in pacman.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// `-y`: refresh the sync index (`-S`).
    pub refresh: u8,
    /// `-u`: upgrade (`-S`), upgradable (`-Q`).
    pub sysupgrade: u8,
    /// `-s`: search (`-S`), status (`-X`).
    pub search: bool,
    /// `-i`: info (`-S`, `-Q`, `-G`, `-P`).
    pub info: u8,
    /// `-l`: list builds (`-S`), files (`-Q`), the latest run log (`-G`).
    pub list: bool,
    /// `-k`: check (`-Q`, `-G`, `-P`), kill (`-X`).
    pub check: u8,
    /// `-c`: clean the cache (`-S`).
    pub clean: u8,
    /// `-w`: download only (`-S`).
    pub download_only: bool,
    /// `-d`: skip the profile check (`-R`), delete (`-P`).
    pub nodeps: u8,
    /// `-e`: edit (`-G`, `-P`).
    pub edit: bool,
    /// `-n`: new profile (`-P`).
    pub new: bool,
    /// `-r <runtime>` for `-Pn`.
    pub runtime: Option<String>,
    pub needed: bool,
    pub pkg: bool,
    pub link: bool,
    pub path: bool,
    pub prefix: Option<PathBuf>,
    pub wait: bool,
    pub debug: bool,
    pub shell: bool,
    /// `-p <profile>` for runs and shells.
    pub profile: Option<String>,
}

/// One modifier, for allow-lists and messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Refresh,
    Sysupgrade,
    Search,
    Info,
    List,
    Check,
    Clean,
    DownloadOnly,
    Nodeps,
    Edit,
    New,
    Runtime,
    Needed,
    Pkg,
    Link,
    Path,
    Prefix,
    Wait,
    Debug,
    Shell,
    Profile,
}

impl Flag {
    fn spelling(self) -> &'static str {
        match self {
            Self::Refresh => "-y",
            Self::Sysupgrade => "-u",
            Self::Search => "-s",
            Self::Info => "-i",
            Self::List => "-l",
            Self::Check => "-k",
            Self::Clean => "-c",
            Self::DownloadOnly => "-w",
            Self::Nodeps => "-d",
            Self::Edit => "-e",
            Self::New => "-n",
            Self::Runtime => "-r",
            Self::Needed => "--needed",
            Self::Pkg => "--pkg",
            Self::Link => "--link",
            Self::Path => "--path",
            Self::Prefix => "--prefix",
            Self::Wait => "--wait",
            Self::Debug => "--debug",
            Self::Shell => "--shell",
            Self::Profile => "-p",
        }
    }
}

impl Modifiers {
    /// Every modifier that was given, in a fixed order.
    fn used(&self) -> Vec<Flag> {
        [
            (Flag::Refresh, self.refresh > 0),
            (Flag::Sysupgrade, self.sysupgrade > 0),
            (Flag::Search, self.search),
            (Flag::Info, self.info > 0),
            (Flag::List, self.list),
            (Flag::Check, self.check > 0),
            (Flag::Clean, self.clean > 0),
            (Flag::DownloadOnly, self.download_only),
            (Flag::Nodeps, self.nodeps > 0),
            (Flag::Edit, self.edit),
            (Flag::New, self.new),
            (Flag::Runtime, self.runtime.is_some()),
            (Flag::Needed, self.needed),
            (Flag::Pkg, self.pkg),
            (Flag::Link, self.link),
            (Flag::Path, self.path),
            (Flag::Prefix, self.prefix.is_some()),
            (Flag::Wait, self.wait),
            (Flag::Debug, self.debug),
            (Flag::Shell, self.shell),
            (Flag::Profile, self.profile.is_some()),
        ]
        .into_iter()
        .filter_map(|(flag, set)| set.then_some(flag))
        .collect()
    }
}

/// What this invocation does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    #[cfg(feature = "download")]
    Sync(Sync),
    Query(Query),
    /// `-R`.
    Remove {
        targets: Vec<RuntimeRef>,
        /// `-d` skips the profile check; `-dd` also removes builds in use.
        nodeps: u8,
    },
    /// `-U`: install from local archives or directories (absolute).
    LocalInstall {
        sources: Vec<PathBuf>,
        link: bool,
    },
    Exec(Exec),
    Games(Games),
    Profiles(Profiles),
    /// `-D`, for the named runtimes or every installed one.
    #[cfg(feature = "diag")]
    Doctor {
        runtimes: Vec<RuntimeRef>,
    },
}

#[cfg(feature = "download")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    /// `-y`: 1 refreshes, 2 forces it.
    pub refresh: u8,
    pub action: SyncAction,
}

#[cfg(feature = "download")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// `-Sy` alone.
    RefreshOnly,
    Install {
        targets: Vec<RuntimeRef>,
        needed: bool,
        download_only: bool,
    },
    /// `-Su [rt…]`; empty upgrades every installed runtime.
    Upgrade {
        targets: Vec<String>,
        /// `-uu`.
        allow_downgrade: bool,
        download_only: bool,
    },
    Search {
        terms: Vec<String>,
        /// `--pkg`: search the package manifests.
        packages: bool,
    },
    Info {
        targets: Vec<RuntimeRef>,
        /// `-ii`: the full package list.
        packages: bool,
    },
    List {
        targets: Vec<String>,
    },
    Clean {
        /// `-cc`: every cached archive.
        all: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    List {
        targets: Vec<RuntimeRef>,
        /// `-q`: names only.
        quiet: bool,
    },
    Info {
        targets: Vec<RuntimeRef>,
    },
    #[cfg(feature = "download")]
    Upgradable {
        targets: Vec<String>,
    },
    Check {
        targets: Vec<RuntimeRef>,
        /// `-kk`: content hashes.
        full: bool,
    },
    Files {
        targets: Vec<RuntimeRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exec {
    Run(Run),
    /// `-X --shell`.
    Shell {
        profile: String,
        debug: bool,
    },
    /// `-Xs`.
    Status,
    /// `-Xk <game|profile|pid>…`.
    Stop {
        targets: Vec<String>,
    },
    /// `-Xkk`.
    StopAll,
}

/// `sruman <target> [args…]` or `-X <target> [args…]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// A game name or a path, resolved later against `games.toml` (D10).
    pub target: String,
    pub args: Vec<String>,
    /// `-p`: the profile for a path, or an override for a game.
    pub profile: Option<String>,
    pub wait: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Games {
    List { targets: Vec<String> },
    Info { targets: Vec<String> },
    Check { targets: Vec<String> },
    Edit,
    Log { game: String, path_only: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profiles {
    List {
        targets: Vec<String>,
    },
    Info {
        targets: Vec<String>,
    },
    Check {
        targets: Vec<String>,
    },
    New {
        name: String,
        runtime: RuntimeRef,
        /// Absolute; `None` uses `prefix_root/<name>`.
        prefix: Option<PathBuf>,
    },
    Edit {
        name: String,
    },
    Delete {
        targets: Vec<String>,
        /// `-dd`: also delete the prefix.
        prefix: bool,
    },
}

impl Operation {
    /// The flags that select this operation, e.g. `-Qk`, for messages.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "download")]
            Self::Sync(sync) => match sync.action {
                SyncAction::RefreshOnly => "-Sy",
                SyncAction::Install { .. } => "-S",
                SyncAction::Upgrade { .. } => "-Su",
                SyncAction::Search { .. } => "-Ss",
                SyncAction::Info { .. } => "-Si",
                SyncAction::List { .. } => "-Sl",
                SyncAction::Clean { .. } => "-Sc",
            },
            Self::Query(query) => match query {
                Query::List { .. } => "-Q",
                Query::Info { .. } => "-Qi",
                #[cfg(feature = "download")]
                Query::Upgradable { .. } => "-Qu",
                Query::Check { .. } => "-Qk",
                Query::Files { .. } => "-Ql",
            },
            Self::Remove { .. } => "-R",
            Self::LocalInstall { .. } => "-U",
            Self::Exec(exec) => match exec {
                Exec::Run(_) => "-X",
                Exec::Shell { .. } => "-X --shell",
                Exec::Status => "-Xs",
                Exec::Stop { .. } => "-Xk",
                Exec::StopAll => "-Xkk",
            },
            Self::Games(games) => match games {
                Games::List { .. } => "-G",
                Games::Info { .. } => "-Gi",
                Games::Check { .. } => "-Gk",
                Games::Edit => "-Ge",
                Games::Log { .. } => "-Gl",
            },
            Self::Profiles(profiles) => match profiles {
                Profiles::List { .. } => "-P",
                Profiles::Info { .. } => "-Pi",
                Profiles::Check { .. } => "-Pk",
                Profiles::New { .. } => "-Pn",
                Profiles::Edit { .. } => "-Pe",
                Profiles::Delete { .. } => "-Pd",
            },
            #[cfg(feature = "diag")]
            Self::Doctor { .. } => "-D",
        }
    }

    /// Whether `--json` applies.
    pub fn supports_json(&self) -> bool {
        match self {
            #[cfg(feature = "download")]
            Self::Sync(sync) => matches!(sync.action, SyncAction::List { .. }),
            Self::Query(query) => matches!(query, Query::List { .. } | Query::Info { .. }),
            Self::Exec(exec) => matches!(exec, Exec::Status),
            Self::Games(games) => matches!(games, Games::List { .. } | Games::Info { .. }),
            Self::Profiles(profiles) => {
                matches!(profiles, Profiles::List { .. } | Profiles::Info { .. })
            }
            _ => false,
        }
    }

    /// Whether this runs something (and so may need a profile).
    pub fn is_run(&self) -> bool {
        matches!(self, Self::Exec(Exec::Run(_) | Exec::Shell { .. }))
    }
}

/// An invalid invocation (exit 2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

/// The parsed command line, before resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invocation {
    pub ops: Vec<OpFlag>,
    pub modifiers: Modifiers,
    pub targets: Vec<String>,
    pub json: bool,
    /// `-q`: for `-Q`, names only.
    pub quiet: bool,
}

/// What resolution needs from outside the command line.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// `default_profile` from `config.toml`, for shells without `-p`.
    pub default_profile: Option<&'a str>,
}

type Result<T> = std::result::Result<T, UsageError>;

fn usage<T>(message: impl Into<String>) -> Result<T> {
    Err(UsageError(message.into()))
}

/// Resolves an invocation into an [`Operation`].
pub fn resolve(inv: Invocation, ctx: &Context) -> Result<Operation> {
    let op = match inv.ops.as_slice() {
        [] => None,
        [op] => Some(*op),
        _ => return usage("only one operation may be used at a time"),
    };

    let used = inv.modifiers.used();
    let allowed = allowed_modifiers(op);
    if let Some(flag) = used.iter().find(|flag| !allowed.contains(flag)) {
        return match op {
            Some(op) => usage(format!(
                "invalid option '{}' for {}",
                flag.spelling(),
                op.spelling()
            )),
            None => usage(format!(
                "invalid option '{}' without an operation (use -h for help)",
                flag.spelling()
            )),
        };
    }

    let operation = match op {
        None => bare(inv, ctx)?,
        #[cfg(feature = "download")]
        Some(OpFlag::Sync) => sync(inv)?,
        Some(OpFlag::Query) => query(inv)?,
        Some(OpFlag::Remove) => remove(inv)?,
        Some(OpFlag::Upgrade) => local_install(inv, ctx)?,
        Some(OpFlag::Exec) => exec(inv, ctx)?,
        Some(OpFlag::Games) => games(inv)?,
        Some(OpFlag::Profiles) => profiles(inv, ctx)?,
        #[cfg(feature = "diag")]
        Some(OpFlag::Doctor) => Operation::Doctor {
            runtimes: runtime_refs(&inv.targets)?,
        },
    };
    Ok(operation)
}

/// Checks `--json` against the resolved operation.
pub fn check_json(operation: &Operation, json: bool) -> Result<()> {
    if json && !operation.supports_json() {
        return usage(format!("invalid option '--json' for {}", operation.name()));
    }
    Ok(())
}

fn allowed_modifiers(op: Option<OpFlag>) -> &'static [Flag] {
    use Flag::*;
    match op {
        None => &[Wait, Debug, Shell, Profile],
        #[cfg(feature = "download")]
        Some(OpFlag::Sync) => &[
            Refresh,
            Sysupgrade,
            Search,
            Info,
            List,
            Clean,
            DownloadOnly,
            Needed,
            Pkg,
        ],
        Some(OpFlag::Query) => {
            if cfg!(feature = "download") {
                &[Info, Sysupgrade, Check, List]
            } else {
                &[Info, Check, List]
            }
        }
        Some(OpFlag::Remove) => &[Nodeps],
        Some(OpFlag::Upgrade) => &[Link],
        Some(OpFlag::Exec) => &[Search, Check, Wait, Debug, Shell, Profile],
        Some(OpFlag::Games) => &[Info, Check, Edit, List, Path],
        Some(OpFlag::Profiles) => &[Info, Check, New, Edit, Nodeps, Runtime, Prefix],
        #[cfg(feature = "diag")]
        Some(OpFlag::Doctor) => &[],
    }
}

/// Rejects more than one of `actions` (the modifiers that pick a
/// sub-operation).
fn exclusive(mods: &Modifiers, actions: &[Flag]) -> Result<()> {
    let set: Vec<Flag> = mods
        .used()
        .into_iter()
        .filter(|flag| actions.contains(flag))
        .collect();
    if let [first, second, ..] = set.as_slice() {
        return usage(format!(
            "invalid option: '{}' and '{}' may not be used together",
            first.spelling(),
            second.spelling()
        ));
    }
    Ok(())
}

/// Rejects modifiers of `op` that don't apply to the sub-operation `name`.
fn only_with(mods: &Modifiers, name: &str, extra: &[Flag], allowed: &[Flag]) -> Result<()> {
    match mods
        .used()
        .into_iter()
        .find(|flag| extra.contains(flag) && !allowed.contains(flag))
    {
        Some(flag) => usage(format!("invalid option '{}' for {name}", flag.spelling())),
        None => Ok(()),
    }
}

fn no_targets(targets: &[String], name: &str) -> Result<()> {
    if targets.is_empty() {
        Ok(())
    } else {
        usage(format!("{name} takes no targets"))
    }
}

fn some_targets(targets: &[String]) -> Result<()> {
    if targets.is_empty() {
        usage("no targets specified (use -h for help)")
    } else {
        Ok(())
    }
}

fn one_target(targets: Vec<String>, name: &str, what: &str) -> Result<String> {
    match <[String; 1]>::try_from(targets) {
        Ok([target]) => Ok(target),
        Err(_) => usage(format!("{name} takes exactly one {what}")),
    }
}

fn runtime_refs(targets: &[String]) -> Result<Vec<RuntimeRef>> {
    targets.iter().map(|target| runtime_ref(target)).collect()
}

fn runtime_ref(target: &str) -> Result<RuntimeRef> {
    target
        .parse()
        .or_else(|err| usage(format!("invalid runtime '{target}': {err}")))
}

/// Runtime names without a `=build`.
#[cfg(feature = "download")]
fn runtime_names(targets: Vec<String>, name: &str) -> Result<Vec<String>> {
    for target in &targets {
        if runtime_ref(target)?.is_pinned() {
            return usage(format!(
                "{name} takes runtime names, not builds: '{target}'"
            ));
        }
    }
    Ok(targets)
}

fn names(targets: Vec<String>, what: &str) -> Result<Vec<String>> {
    targets
        .into_iter()
        .map(|target| name(target, what))
        .collect()
}

fn name(target: String, what: &str) -> Result<String> {
    match model::validate_name(&target) {
        Ok(()) => Ok(target),
        Err(err) => usage(format!("invalid {what}: {err}")),
    }
}

/// A path from the command line: `~` expanded (for quoted ones), relative
/// to the working directory.
fn cli_path(path: &Path, ctx: &Context) -> PathBuf {
    ctx.cwd.join(paths::expand_tilde(path, ctx.home))
}

/// No operation: run a target, or `--shell`. Bare `sruman` (= `-Syu`) is
/// handled before this, in the settings resolver.
fn bare(inv: Invocation, ctx: &Context) -> Result<Operation> {
    if inv.targets.is_empty() && !inv.modifiers.shell {
        #[cfg(feature = "download")]
        if inv.modifiers.used().is_empty() {
            return Ok(Operation::Sync(Sync {
                refresh: 1,
                action: SyncAction::Upgrade {
                    targets: Vec::new(),
                    allow_downgrade: false,
                    download_only: false,
                },
            }));
        }
        return usage("no operation specified (use -h for help)");
    }
    exec(inv, ctx)
}

#[cfg(feature = "download")]
fn sync(inv: Invocation) -> Result<Operation> {
    let mods = &inv.modifiers;
    exclusive(
        mods,
        &[
            Flag::Search,
            Flag::Info,
            Flag::List,
            Flag::Clean,
            Flag::Sysupgrade,
        ],
    )?;
    let extras = [Flag::DownloadOnly, Flag::Needed, Flag::Pkg];
    let targets = inv.targets;

    let action = if mods.search {
        only_with(mods, "-Ss", &extras, &[Flag::Pkg])?;
        SyncAction::Search {
            terms: targets,
            packages: mods.pkg,
        }
    } else if mods.info > 0 {
        only_with(mods, "-Si", &extras, &[])?;
        SyncAction::Info {
            targets: runtime_refs(&targets)?,
            packages: mods.info > 1,
        }
    } else if mods.list {
        only_with(mods, "-Sl", &extras, &[])?;
        SyncAction::List {
            targets: runtime_names(targets, "-Sl")?,
        }
    } else if mods.clean > 0 {
        only_with(mods, "-Sc", &extras, &[])?;
        no_targets(&targets, "-Sc")?;
        SyncAction::Clean {
            all: mods.clean > 1,
        }
    } else if mods.sysupgrade > 0 {
        only_with(mods, "-Su", &extras, &[Flag::DownloadOnly])?;
        SyncAction::Upgrade {
            targets: runtime_names(targets, "-Su")?,
            allow_downgrade: mods.sysupgrade > 1,
            download_only: mods.download_only,
        }
    } else if targets.is_empty() && mods.refresh > 0 {
        only_with(mods, "-Sy", &extras, &[])?;
        SyncAction::RefreshOnly
    } else {
        only_with(mods, "-S", &extras, &[Flag::DownloadOnly, Flag::Needed])?;
        some_targets(&targets)?;
        SyncAction::Install {
            targets: runtime_refs(&targets)?,
            needed: mods.needed,
            download_only: mods.download_only,
        }
    };
    Ok(Operation::Sync(Sync {
        refresh: mods.refresh.min(2),
        action,
    }))
}

fn query(inv: Invocation) -> Result<Operation> {
    let mods = &inv.modifiers;
    exclusive(
        mods,
        &[Flag::Info, Flag::Sysupgrade, Flag::Check, Flag::List],
    )?;
    let targets = inv.targets;

    let query = if mods.info > 0 {
        Query::Info {
            targets: runtime_refs(&targets)?,
        }
    } else if mods.check > 0 {
        Query::Check {
            targets: runtime_refs(&targets)?,
            full: mods.check > 1,
        }
    } else if mods.list {
        some_targets(&targets)?;
        Query::Files {
            targets: runtime_refs(&targets)?,
        }
    } else if mods.sysupgrade > 0 {
        #[cfg(feature = "download")]
        {
            Query::Upgradable {
                targets: runtime_names(targets, "-Qu")?,
            }
        }
        #[cfg(not(feature = "download"))]
        unreachable!("-u is not allowed for -Q without the download feature")
    } else {
        Query::List {
            targets: runtime_refs(&targets)?,
            quiet: inv.quiet,
        }
    };
    Ok(Operation::Query(query))
}

fn remove(inv: Invocation) -> Result<Operation> {
    some_targets(&inv.targets)?;
    Ok(Operation::Remove {
        targets: runtime_refs(&inv.targets)?,
        nodeps: inv.modifiers.nodeps.min(2),
    })
}

fn local_install(inv: Invocation, ctx: &Context) -> Result<Operation> {
    some_targets(&inv.targets)?;
    Ok(Operation::LocalInstall {
        sources: inv
            .targets
            .iter()
            .map(|target| cli_path(Path::new(target), ctx))
            .collect(),
        link: inv.modifiers.link,
    })
}

fn exec(inv: Invocation, ctx: &Context) -> Result<Operation> {
    let mods = &inv.modifiers;
    exclusive(mods, &[Flag::Search, Flag::Check, Flag::Shell])?;
    let extras = [Flag::Wait, Flag::Debug, Flag::Profile];
    let mut targets = inv.targets;

    let exec = if mods.search {
        only_with(mods, "-Xs", &extras, &[])?;
        no_targets(&targets, "-Xs")?;
        Exec::Status
    } else if mods.check > 1 {
        only_with(mods, "-Xkk", &extras, &[])?;
        no_targets(&targets, "-Xkk")?;
        Exec::StopAll
    } else if mods.check > 0 {
        only_with(mods, "-Xk", &extras, &[])?;
        some_targets(&targets)?;
        Exec::Stop { targets }
    } else if mods.shell {
        only_with(mods, "--shell", &extras, &[Flag::Debug, Flag::Profile])?;
        no_targets(&targets, "--shell")?;
        let profile = match (&mods.profile, ctx.default_profile) {
            (Some(profile), _) => profile.clone(),
            (None, Some(profile)) => profile.to_owned(),
            (None, None) => {
                return usage(
                    "--shell needs a profile: -p <profile>, or default_profile in config.toml",
                );
            }
        };
        Exec::Shell {
            profile: name(profile, "profile")?,
            debug: mods.debug,
        }
    } else {
        some_targets(&targets)?;
        let target = targets.remove(0);
        Exec::Run(Run {
            target,
            args: targets,
            profile: mods
                .profile
                .clone()
                .map(|profile| name(profile, "profile"))
                .transpose()?,
            wait: mods.wait,
            debug: mods.debug,
        })
    };
    Ok(Operation::Exec(exec))
}

fn games(inv: Invocation) -> Result<Operation> {
    let mods = &inv.modifiers;
    exclusive(mods, &[Flag::Info, Flag::Check, Flag::Edit, Flag::List])?;
    let targets = names(inv.targets, "game")?;

    let (name, games) = if mods.list {
        let game = one_target(targets, "-Gl", "game")?;
        (
            "-Gl",
            Games::Log {
                game,
                path_only: mods.path,
            },
        )
    } else if mods.info > 0 {
        some_targets(&targets)?;
        ("-Gi", Games::Info { targets })
    } else if mods.check > 0 {
        ("-Gk", Games::Check { targets })
    } else if mods.edit {
        no_targets(&targets, "-Ge")?;
        ("-Ge", Games::Edit)
    } else {
        ("-G", Games::List { targets })
    };
    if !matches!(games, Games::Log { .. }) {
        only_with(mods, name, &[Flag::Path], &[])?;
    }
    Ok(Operation::Games(games))
}

fn profiles(inv: Invocation, ctx: &Context) -> Result<Operation> {
    let mods = &inv.modifiers;
    exclusive(
        mods,
        &[Flag::Info, Flag::Check, Flag::New, Flag::Edit, Flag::Nodeps],
    )?;
    let targets = names(inv.targets, "profile")?;

    if mods.new {
        let name = one_target(targets, "-Pn", "profile")?;
        let Some(runtime) = &mods.runtime else {
            return usage(format!(
                "-Pn needs a runtime: sruman -Pn {name} -r <runtime>"
            ));
        };
        return Ok(Operation::Profiles(Profiles::New {
            name,
            runtime: runtime_ref(runtime)?,
            prefix: mods.prefix.as_deref().map(|prefix| cli_path(prefix, ctx)),
        }));
    }

    let (name, profiles) = if mods.info > 0 {
        ("-Pi", Profiles::Info { targets })
    } else if mods.check > 0 {
        ("-Pk", Profiles::Check { targets })
    } else if mods.edit {
        let name = one_target(targets, "-Pe", "profile")?;
        ("-Pe", Profiles::Edit { name })
    } else if mods.nodeps > 0 {
        some_targets(&targets)?;
        (
            "-Pd",
            Profiles::Delete {
                targets,
                prefix: mods.nodeps > 1,
            },
        )
    } else {
        ("-P", Profiles::List { targets })
    };
    only_with(mods, name, &[Flag::Runtime, Flag::Prefix], &[])?;
    Ok(Operation::Profiles(profiles))
}
