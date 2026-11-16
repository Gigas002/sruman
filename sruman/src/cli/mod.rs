//! Command-line definitions: pacman's grammar (one uppercase operation,
//! lowercase modifiers, then targets). Parsed only in `main`, then
//! converted into [`CliOptions`] so no `clap` type crosses the settings
//! boundary; `settings` checks which modifiers fit which operation.

use std::{ffi::OsString, path::PathBuf};

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, ValueEnum};

use crate::{
    config::ColorChoice,
    settings::{CliOptions, Invocation, Modifiers, OpFlag},
};

#[cfg(test)]
mod tests;

const OPERATIONS: &str = "Operations";
const MODIFIERS: &str = "Modifiers";

/// Steam Linux Runtime manager: install and update runtimes, run native
/// games in them through profiles and prefixes.
///
/// Without arguments, sruman refreshes and upgrades every runtime (-Syu).
/// A bare target runs: a game from games.toml, or a path with -p.
#[derive(Debug, Parser)]
#[command(
    name = "sruman",
    version,
    about,
    long_about,
    override_usage = "sruman <operation> [options] [targets]\n       \
                      sruman [-p <profile>] <game|path> [args...]"
)]
pub struct Cli {
    /// Sync: install, refresh, upgrade, search runtimes.
    #[cfg(feature = "download")]
    #[arg(short = 'S', long = "sync", help_heading = OPERATIONS)]
    pub sync: bool,

    /// Query installed runtimes.
    #[arg(short = 'Q', long = "query", help_heading = OPERATIONS)]
    pub query: bool,

    /// Remove runtimes.
    #[arg(short = 'R', long = "remove", help_heading = OPERATIONS)]
    pub remove: bool,

    /// Install from a local archive or directory.
    #[arg(short = 'U', long = "upgrade", help_heading = OPERATIONS)]
    pub upgrade: bool,

    /// Execute: run a game or path, open a shell, list or stop sessions.
    #[arg(short = 'X', long = "exec", help_heading = OPERATIONS)]
    pub exec: bool,

    /// Games from games.toml.
    #[arg(short = 'G', long = "games", help_heading = OPERATIONS)]
    pub games: bool,

    /// Profiles and their prefixes.
    #[arg(short = 'P', long = "profiles", help_heading = OPERATIONS)]
    pub profiles: bool,

    /// Doctor: check user namespaces, bubblewrap and the runtimes.
    #[cfg(feature = "diag")]
    #[arg(short = 'D', long = "doctor", help_heading = OPERATIONS)]
    pub doctor: bool,

    /// -S: refresh the sync index (-yy: force).
    #[arg(short = 'y', long = "refresh", action = ArgAction::Count, help_heading = MODIFIERS)]
    pub refresh: u8,

    /// -S: upgrade installed runtimes (-uu: allow downgrades). -Q: upgradable.
    #[arg(short = 'u', long = "sysupgrade", action = ArgAction::Count, help_heading = MODIFIERS)]
    pub sysupgrade: u8,

    /// -S: search runtimes. -X: running sessions.
    #[arg(short = 's', help_heading = MODIFIERS)]
    pub search: bool,

    /// -S, -Q, -G, -P: show info (-Sii: with packages).
    #[arg(short = 'i', long = "info", action = ArgAction::Count, help_heading = MODIFIERS)]
    pub info: u8,

    /// -S: list builds. -Q: list files. -G: print the latest run log.
    #[arg(short = 'l', long = "list", help_heading = MODIFIERS)]
    pub list: bool,

    /// -Q, -G, -P: check (-Qkk: hashes). -X: stop sessions (-Xkk: all).
    #[arg(short = 'k', action = ArgAction::Count, help_heading = MODIFIERS)]
    pub check: u8,

    /// -S: clean the download cache (-cc: all of it).
    #[arg(short = 'c', long = "clean", action = ArgAction::Count, help_heading = MODIFIERS)]
    pub clean: u8,

    /// -S: download only.
    #[arg(short = 'w', long = "downloadonly", help_heading = MODIFIERS)]
    pub download_only: bool,

    /// -R: skip the profile check (-dd: remove builds in use). -P: delete (-dd: and the prefix).
    #[arg(short = 'd', action = ArgAction::Count, help_heading = MODIFIERS)]
    pub nodeps: u8,

    /// -G, -P: edit in $EDITOR.
    #[arg(short = 'e', long = "edit", help_heading = MODIFIERS)]
    pub edit: bool,

    /// -P: new profile.
    #[arg(short = 'n', long = "new", help_heading = MODIFIERS)]
    pub new: bool,

    /// -Pn: the profile's runtime.
    #[arg(short = 'r', long = "runtime", value_name = "RUNTIME", help_heading = MODIFIERS)]
    pub runtime: Option<String>,

    /// -Pn: the profile's prefix directory.
    #[arg(long, value_name = "DIR", help_heading = MODIFIERS)]
    pub prefix: Option<PathBuf>,

    /// -S: skip installed runtimes.
    #[arg(long, help_heading = MODIFIERS)]
    pub needed: bool,

    /// -Ss: search package manifests.
    #[arg(long, help_heading = MODIFIERS)]
    pub pkg: bool,

    /// -U: register a directory in place, without copying.
    #[arg(long, help_heading = MODIFIERS)]
    pub link: bool,

    /// -Gl: print the log's path.
    #[arg(long, help_heading = MODIFIERS)]
    pub path: bool,

    /// Run with this profile.
    #[arg(short = 'p', long = "profile", value_name = "PROFILE", help_heading = MODIFIERS)]
    pub profile: Option<String>,

    /// Run: stay attached and exit with the game's code.
    #[arg(long, help_heading = MODIFIERS)]
    pub wait: bool,

    /// Run: verbose runtime logging.
    #[arg(long, help_heading = MODIFIERS)]
    pub debug: bool,

    /// Open a shell in the profile's runtime and prefix.
    #[arg(long, help_heading = MODIFIERS)]
    pub shell: bool,

    /// Answer every prompt with its default.
    #[arg(long)]
    pub noconfirm: bool,

    /// Colorize the output.
    #[arg(long, value_name = "WHEN", value_enum)]
    pub color: Option<ColorArg>,

    /// Less output (-Q: names only).
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Log to stderr (-vv: more).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    pub verbose: u8,

    /// Use this config.toml.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Print JSON (-Q, -Qi, -P, -Pi, -G, -Gi, -Xs, -Sl).
    #[cfg(feature = "json")]
    #[arg(long)]
    pub json: bool,

    /// Runtimes, profiles or games; for a run, the game or path, then its
    /// arguments.
    #[arg(value_name = "TARGETS", trailing_var_arg = true)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorArg {
    Auto,
    Always,
    Never,
}

impl From<ColorArg> for ColorChoice {
    fn from(color: ColorArg) -> Self {
        match color {
            ColorArg::Auto => Self::Auto,
            ColorArg::Always => Self::Always,
            ColorArg::Never => Self::Never,
        }
    }
}

impl Cli {
    /// Parses `args` (the first is the program name).
    ///
    /// A run passes everything after its target to the game untouched,
    /// `-` options included. Other operations take options after their
    /// targets too (`-S sniper --needed`), as pacman does, so they are parsed
    /// again without that pass-through.
    pub fn try_parse_args<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
        let cli = Self::try_parse_from(&args)?;
        if cli.is_run() || !cli.targets.iter().any(|target| target.starts_with('-')) {
            return Ok(cli);
        }

        let mut command =
            Self::command().mut_arg("targets", |targets| targets.trailing_var_arg(false));
        let matches = command.try_get_matches_from_mut(&args)?;
        Self::from_arg_matches(&matches).map_err(|err| err.format(&mut command))
    }

    /// Whether the targets are a run target plus its arguments.
    fn is_run(&self) -> bool {
        let operations = self.operations();
        let exec = matches!(operations.as_slice(), [] | [OpFlag::Exec]);
        exec && !self.search && self.check == 0
    }

    fn operations(&self) -> Vec<OpFlag> {
        let mut ops = Vec::new();
        #[cfg(feature = "download")]
        if self.sync {
            ops.push(OpFlag::Sync);
        }
        for (set, op) in [
            (self.query, OpFlag::Query),
            (self.remove, OpFlag::Remove),
            (self.upgrade, OpFlag::Upgrade),
            (self.exec, OpFlag::Exec),
            (self.games, OpFlag::Games),
            (self.profiles, OpFlag::Profiles),
        ] {
            if set {
                ops.push(op);
            }
        }
        #[cfg(feature = "diag")]
        if self.doctor {
            ops.push(OpFlag::Doctor);
        }
        ops
    }
}

impl From<Cli> for CliOptions {
    fn from(cli: Cli) -> Self {
        let ops = cli.operations();
        #[cfg(feature = "json")]
        let json = cli.json;
        #[cfg(not(feature = "json"))]
        let json = false;

        Self {
            invocation: Invocation {
                ops,
                modifiers: Modifiers {
                    refresh: cli.refresh,
                    sysupgrade: cli.sysupgrade,
                    search: cli.search,
                    info: cli.info,
                    list: cli.list,
                    check: cli.check,
                    clean: cli.clean,
                    download_only: cli.download_only,
                    nodeps: cli.nodeps,
                    edit: cli.edit,
                    new: cli.new,
                    runtime: cli.runtime,
                    needed: cli.needed,
                    pkg: cli.pkg,
                    link: cli.link,
                    path: cli.path,
                    prefix: cli.prefix,
                    wait: cli.wait,
                    debug: cli.debug,
                    shell: cli.shell,
                    profile: cli.profile,
                },
                targets: cli.targets,
                json,
                quiet: cli.quiet,
            },
            config: cli.config,
            color: cli.color.map(Into::into),
            noconfirm: cli.noconfirm,
            verbose: cli.verbose,
        }
    }
}
