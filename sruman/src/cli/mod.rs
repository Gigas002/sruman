//! Command-line definitions: subcommands in lootrs' and gaki's style (D9).
//! Parsed only in `main`, then converted into [`CliOptions`] so no `clap`
//! type crosses the settings boundary. Each command has only its own
//! options, so clap rejects the rest; runtime refs and names are checked
//! while parsing.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use libsruman::{
    model::{self, SlrMode},
    version::RuntimeRef,
};

use crate::{
    config::ColorChoice,
    settings::{
        CliOptions,
        command::{Command as Cmd, Execute, LaunchMode, Profiles, Run},
    },
};

#[cfg(test)]
mod tests;

/// Steam Linux Runtime manager: install and update runtimes, run native
/// games in them through profiles and prefixes.
#[derive(Debug, Parser)]
#[command(name = "sruman", version, about, arg_required_else_help = true)]
pub struct Cli {
    /// Answer every prompt with its default.
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Colorize the output.
    #[arg(long, value_name = "WHEN", value_enum, global = true)]
    pub color: Option<ColorArg>,

    /// Less output (`runtimes`: names only).
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Log to stderr (-vv: more).
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Use this config.toml.
    #[arg(long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

/// `--json`, on the commands that print data.
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct Json {
    /// Print JSON.
    #[cfg(feature = "json")]
    #[arg(long)]
    pub json: bool,
}

impl Json {
    fn get(self) -> bool {
        #[cfg(feature = "json")]
        return self.json;
        #[cfg(not(feature = "json"))]
        false
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Refresh the sync index.
    #[cfg(feature = "download")]
    Sync {
        /// Refetch even if nothing changed on the server.
        #[arg(long)]
        force: bool,
    },
    /// Install runtimes, with their dependencies.
    #[cfg(feature = "download")]
    Install {
        /// Runtimes, optionally pinned: `sniper` or `sniper=<build>`.
        #[arg(required = true, value_parser = runtime_ref)]
        runtimes: Vec<RuntimeRef>,
        /// Skip runtimes that are installed already.
        #[arg(long)]
        needed: bool,
        /// Download without installing.
        #[arg(long)]
        download_only: bool,
    },
    /// Upgrade installed runtimes (all, or the named ones).
    #[cfg(feature = "download")]
    Upgrade {
        #[arg(value_parser = runtime_name)]
        runtimes: Vec<String>,
        /// Refresh the sync index first.
        #[arg(long)]
        sync: bool,
        /// Go back to the channel's build when an installed one is newer.
        #[arg(long)]
        allow_downgrade: bool,
        /// Download without installing.
        #[arg(long)]
        download_only: bool,
    },
    /// Installed runtimes with a newer build in the sync index.
    #[cfg(feature = "download")]
    Outdated {
        #[arg(value_parser = runtime_name)]
        runtimes: Vec<String>,
        #[command(flatten)]
        json: Json,
    },
    /// Remove runtimes.
    Remove {
        #[arg(required = true, value_parser = runtime_ref)]
        runtimes: Vec<RuntimeRef>,
        /// Remove even if a profile uses it (never a build in use).
        #[arg(long)]
        force: bool,
    },
    /// Search runtimes, or their packages.
    #[cfg(feature = "download")]
    Search {
        /// Regular expressions; none lists everything.
        terms: Vec<String>,
        /// Search the package manifests.
        #[arg(long)]
        packages: bool,
    },
    /// Show a runtime: build, channel, sizes, dependencies, users.
    Info {
        #[arg(required = true, value_parser = runtime_ref)]
        runtimes: Vec<RuntimeRef>,
        /// List its packages.
        #[arg(long)]
        packages: bool,
        #[command(flatten)]
        json: Json,
    },
    /// List installed runtimes.
    Runtimes {
        #[command(flatten)]
        json: Json,
    },
    /// List a runtime's builds on the server.
    #[cfg(feature = "download")]
    Builds {
        #[arg(value_parser = runtime_name)]
        runtime: String,
        #[command(flatten)]
        json: Json,
    },
    /// List an installed build's files.
    Files {
        #[arg(value_parser = runtime_ref)]
        runtime: RuntimeRef,
    },
    /// Check installed builds against their file lists (all without arguments).
    Verify {
        #[arg(value_parser = runtime_ref)]
        runtimes: Vec<RuntimeRef>,
        /// Check content hashes too.
        #[arg(long)]
        full: bool,
    },
    /// Clean the download cache and old builds.
    #[cfg(feature = "download")]
    Clean {
        /// Every cached archive.
        #[arg(long)]
        all: bool,
    },
    /// Install from local archives or directories.
    Import {
        #[arg(required = true)]
        sources: Vec<PathBuf>,
        /// Register a directory in place, without copying.
        #[arg(long)]
        link: bool,
    },

    /// Run a game from games.toml.
    Run {
        #[arg(value_parser = name)]
        game: String,
        /// Use this profile for this run.
        #[arg(short, long, value_parser = name)]
        profile: Option<String>,
        /// Stay attached: stream output and exit with the game's code.
        #[arg(long)]
        wait: bool,
        /// Verbose runtime logging for this run.
        #[arg(long)]
        debug: bool,
        /// Internal: run as the detached supervisor.
        #[arg(long, hide = true, conflicts_with = "wait")]
        supervised: bool,
        /// Extra arguments appended to the game's own.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run an executable once with a profile.
    Execute {
        /// The profile [default: default_profile from config.toml].
        #[arg(short, long, value_parser = name)]
        profile: Option<String>,
        /// A path, or a program in the runtime's PATH.
        target: PathBuf,
        /// Stay attached: stream output and exit with the program's code.
        #[arg(long)]
        wait: bool,
        /// Verbose runtime logging for this run.
        #[arg(long)]
        debug: bool,
        /// Internal: run as the detached supervisor.
        #[arg(long, hide = true, conflicts_with = "wait")]
        supervised: bool,
        /// Arguments for the executable.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Open a shell in a profile's runtime and prefix.
    Shell {
        /// The profile [default: default_profile from config.toml].
        #[arg(short, long, value_parser = name)]
        profile: Option<String>,
        /// Verbose runtime logging.
        #[arg(long)]
        debug: bool,
    },
    /// Show running games and sessions.
    Status {
        #[command(flatten)]
        json: Json,
    },
    /// Stop games, profiles' sessions or processes.
    Stop {
        /// A game, a profile or a pid.
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Stop everything sruman started.
    Killall,
    /// List games.
    List {
        #[command(flatten)]
        json: Json,
    },
    /// Show a game: executable, profile, runtime, prefix, environment.
    Show {
        #[arg(value_parser = name)]
        game: String,
        #[command(flatten)]
        json: Json,
    },
    /// Check games and profiles without launching (all without arguments).
    Check {
        /// Games or profiles.
        #[arg(value_parser = name)]
        targets: Vec<String>,
    },
    /// Print a game's latest run log.
    Logs {
        #[arg(value_parser = name)]
        game: String,
        /// Print the log's path instead.
        #[arg(long)]
        path: bool,
    },
    /// Create or update a profile's prefix.
    Init {
        #[arg(value_parser = name)]
        profile: String,
    },
    /// Manage profiles.
    #[command(subcommand)]
    Profiles(ProfilesCommand),
    /// Check user namespaces, bubblewrap and the runtimes.
    #[cfg(feature = "diag")]
    Doctor {
        /// Runtimes to check; all installed ones without arguments.
        #[arg(value_parser = runtime_ref)]
        runtimes: Vec<RuntimeRef>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProfilesCommand {
    /// List profiles, implicit ones included.
    List {
        #[command(flatten)]
        json: Json,
    },
    /// Show a profile: runtime build, mode, prefix and who shares it.
    Show {
        #[arg(value_parser = name)]
        profile: String,
        #[command(flatten)]
        json: Json,
    },
    /// Create a profile, then its prefix.
    New {
        #[arg(value_parser = name)]
        profile: String,
        /// Its runtime: `sniper` or `sniper=<build>`.
        #[arg(short, long, value_parser = runtime_ref)]
        runtime: RuntimeRef,
        /// Where its libraries come from.
        #[arg(long, value_enum, default_value_t = ModeArg::Container)]
        mode: ModeArg,
        /// Its prefix directory [default: prefix_root/PROFILE].
        #[arg(long, value_name = "DIR")]
        prefix: Option<PathBuf>,
    },
    /// Edit a profile in $EDITOR, then check it.
    Edit {
        #[arg(value_parser = name)]
        profile: String,
    },
    /// Edit games.toml in $EDITOR, then check it.
    EditGames,
    /// Delete profiles.
    Delete {
        #[arg(required = true, value_parser = name)]
        profiles: Vec<String>,
        /// Also delete the prefix, unless it's shared or in use.
        #[arg(long)]
        prefix: bool,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    Container,
    Overlay,
}

impl From<ModeArg> for SlrMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Container => Self::Container,
            ModeArg::Overlay => Self::Overlay,
        }
    }
}

fn runtime_ref(text: &str) -> Result<RuntimeRef, String> {
    text.parse().map_err(|e| format!("{e}"))
}

/// A runtime name without a `=build`.
#[cfg(feature = "download")]
fn runtime_name(text: &str) -> Result<String, String> {
    let runtime = runtime_ref(text)?;
    if runtime.is_pinned() {
        return Err("takes runtime names, not builds".to_owned());
    }
    Ok(text.to_owned())
}

/// A game or profile name.
fn name(text: &str) -> Result<String, String> {
    model::validate_name(text)?;
    Ok(text.to_owned())
}

impl From<Cli> for CliOptions {
    fn from(cli: Cli) -> Self {
        let (command, json) = command(cli.command, cli.quiet);
        Self {
            command,
            json,
            quiet: cli.quiet,
            config: cli.config,
            color: cli.color.map(Into::into),
            yes: cli.yes,
            verbose: cli.verbose,
        }
    }
}

/// The settings' command and whether `--json` was given.
fn command(command: Command, quiet: bool) -> (Cmd, bool) {
    let plain = |cmd| (cmd, false);
    match command {
        #[cfg(feature = "download")]
        Command::Sync { force } => plain(Cmd::Sync { force }),
        #[cfg(feature = "download")]
        Command::Install {
            runtimes,
            needed,
            download_only,
        } => plain(Cmd::Install {
            targets: runtimes,
            needed,
            download_only,
        }),
        #[cfg(feature = "download")]
        Command::Upgrade {
            runtimes,
            sync,
            allow_downgrade,
            download_only,
        } => plain(Cmd::Upgrade {
            targets: runtimes,
            sync,
            allow_downgrade,
            download_only,
        }),
        #[cfg(feature = "download")]
        Command::Outdated { runtimes, json } => (Cmd::Outdated { targets: runtimes }, json.get()),
        Command::Remove { runtimes, force } => plain(Cmd::Remove {
            targets: runtimes,
            force,
        }),
        #[cfg(feature = "download")]
        Command::Search { terms, packages } => plain(Cmd::Search { terms, packages }),
        Command::Info {
            runtimes,
            packages,
            json,
        } => (
            Cmd::Info {
                targets: runtimes,
                packages,
            },
            json.get(),
        ),
        Command::Runtimes { json } => (Cmd::Runtimes { quiet }, json.get()),
        #[cfg(feature = "download")]
        Command::Builds { runtime, json } => (Cmd::Builds { runtime }, json.get()),
        Command::Files { runtime } => plain(Cmd::Files { runtime }),
        Command::Verify { runtimes, full } => plain(Cmd::Verify {
            targets: runtimes,
            full,
        }),
        #[cfg(feature = "download")]
        Command::Clean { all } => plain(Cmd::Clean { all }),
        Command::Import { sources, link } => plain(Cmd::Import { sources, link }),
        Command::Run {
            game,
            profile,
            wait,
            debug,
            supervised,
            args,
        } => plain(Cmd::Run(Run {
            game,
            args,
            profile,
            mode: LaunchMode::new(wait, supervised),
            debug,
        })),
        Command::Execute {
            profile,
            target,
            wait,
            debug,
            supervised,
            args,
        } => plain(Cmd::Execute(Execute {
            target,
            args,
            profile,
            mode: LaunchMode::new(wait, supervised),
            debug,
        })),
        Command::Shell { profile, debug } => plain(Cmd::Shell { profile, debug }),
        Command::Status { json } => (Cmd::Status, json.get()),
        Command::Stop { targets } => plain(Cmd::Stop { targets }),
        Command::Killall => plain(Cmd::Killall),
        Command::List { json } => (Cmd::List, json.get()),
        Command::Show { game, json } => (Cmd::Show { game }, json.get()),
        Command::Check { targets } => plain(Cmd::Check { targets }),
        Command::Logs { game, path } => plain(Cmd::Logs {
            game,
            path_only: path,
        }),
        Command::Init { profile } => plain(Cmd::Init { profile }),
        Command::Profiles(profiles) => match profiles {
            ProfilesCommand::List { json } => (Cmd::Profiles(Profiles::List), json.get()),
            ProfilesCommand::Show { profile, json } => {
                (Cmd::Profiles(Profiles::Show { name: profile }), json.get())
            }
            ProfilesCommand::New {
                profile,
                runtime,
                mode,
                prefix,
            } => plain(Cmd::Profiles(Profiles::New {
                name: profile,
                runtime,
                mode: mode.into(),
                prefix,
            })),
            ProfilesCommand::Edit { profile } => {
                plain(Cmd::Profiles(Profiles::Edit { name: profile }))
            }
            ProfilesCommand::EditGames => plain(Cmd::Profiles(Profiles::EditGames)),
            ProfilesCommand::Delete { profiles, prefix } => {
                plain(Cmd::Profiles(Profiles::Delete {
                    names: profiles,
                    prefix,
                }))
            }
        },
        #[cfg(feature = "diag")]
        Command::Doctor { runtimes } => plain(Cmd::Doctor { runtimes }),
    }
}
