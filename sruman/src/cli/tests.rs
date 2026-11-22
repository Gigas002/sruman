//! The CLI tables of `docs/PLAN.md`, row by row: what each command line
//! parses to, and what clap refuses.

use clap::{Parser, error::ErrorKind};
use libsruman::version::RuntimeRef;

use super::*;

fn parse(args: &[&str]) -> Result<CliOptions, clap::Error> {
    Cli::try_parse_from(std::iter::once("sruman").chain(args.iter().copied())).map(Into::into)
}

/// The command `args` parse to, and `--json`.
fn cmd(args: &[&str]) -> (Cmd, bool) {
    let options = parse(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
    (options.command, options.json)
}

fn plain(args: &[&str]) -> Cmd {
    let (command, json) = cmd(args);
    assert!(!json, "{args:?}");
    command
}

fn error(args: &[&str]) -> clap::Error {
    match parse(args) {
        Ok(options) => panic!("{args:?} parsed: {:?}", options.command),
        Err(err) => err,
    }
}

fn rt(text: &str) -> RuntimeRef {
    text.parse().unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn bare_sruman_prints_the_help() {
    let err = error(&[]);
    assert_eq!(
        err.kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    assert_eq!(err.exit_code(), 2);
    let help = err.render().to_string();
    assert!(help.contains("Usage: sruman [OPTIONS] <COMMAND>"), "{help}");
}

#[cfg(feature = "download")]
#[test]
fn runtime_commands_with_downloads() {
    assert_eq!(plain(&["sync"]), Cmd::Sync { force: false });
    assert_eq!(plain(&["sync", "--force"]), Cmd::Sync { force: true });
    assert_eq!(
        plain(&["install", "scout", "sniper=3.0.1", "--needed"]),
        Cmd::Install {
            targets: vec![rt("scout"), rt("sniper=3.0.1")],
            needed: true,
            download_only: false,
        }
    );
    assert_eq!(
        plain(&["install", "--download-only", "steamrt4"]),
        Cmd::Install {
            targets: vec![rt("steamrt4")],
            needed: false,
            download_only: true,
        }
    );
    assert_eq!(
        plain(&["upgrade"]),
        Cmd::Upgrade {
            targets: Vec::new(),
            sync: false,
            allow_downgrade: false,
            download_only: false,
        }
    );
    assert_eq!(
        plain(&[
            "upgrade",
            "sniper",
            "--sync",
            "--allow-downgrade",
            "--download-only"
        ]),
        Cmd::Upgrade {
            targets: strings(&["sniper"]),
            sync: true,
            allow_downgrade: true,
            download_only: true,
        }
    );
    assert_eq!(
        plain(&["outdated"]),
        Cmd::Outdated {
            targets: Vec::new()
        }
    );
    assert_eq!(
        plain(&["search", "snip", "rt[0-9]"]),
        Cmd::Search {
            terms: strings(&["snip", "rt[0-9]"]),
            packages: false,
        }
    );
    assert_eq!(
        plain(&["search", "--packages", "mesa"]),
        Cmd::Search {
            terms: strings(&["mesa"]),
            packages: true,
        }
    );
    assert_eq!(
        plain(&["builds", "sniper"]),
        Cmd::Builds {
            runtime: "sniper".into()
        }
    );
    assert_eq!(plain(&["clean"]), Cmd::Clean { all: false });
    assert_eq!(plain(&["clean", "--all"]), Cmd::Clean { all: true });

    // Names only where a build makes no sense.
    for args in [
        &["upgrade", "sniper=3.0.1"][..],
        &["outdated", "sniper=3.0.1"],
        &["builds", "sniper=3.0.1"],
    ] {
        let message = error(args).render().to_string();
        assert!(
            message.contains("takes runtime names, not builds"),
            "{message}"
        );
    }
    assert_eq!(
        error(&["install"]).kind(),
        ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn runtime_commands() {
    assert_eq!(
        plain(&["remove", "scout", "soldier=2.0.1"]),
        Cmd::Remove {
            targets: vec![rt("scout"), rt("soldier=2.0.1")],
            force: false,
        }
    );
    assert_eq!(
        plain(&["remove", "--force", "scout"]),
        Cmd::Remove {
            targets: vec![rt("scout")],
            force: true,
        }
    );
    assert_eq!(
        plain(&["info", "sniper=3.0.1", "--packages"]),
        Cmd::Info {
            targets: vec![rt("sniper=3.0.1")],
            packages: true,
        }
    );
    assert_eq!(plain(&["runtimes"]), Cmd::Runtimes { quiet: false });
    assert_eq!(plain(&["runtimes", "-q"]), Cmd::Runtimes { quiet: true });
    assert_eq!(plain(&["-q", "runtimes"]), Cmd::Runtimes { quiet: true });
    assert_eq!(
        plain(&["files", "sniper"]),
        Cmd::Files {
            runtime: rt("sniper")
        }
    );
    assert_eq!(
        plain(&["verify"]),
        Cmd::Verify {
            targets: Vec::new(),
            full: false,
        }
    );
    assert_eq!(
        plain(&["verify", "sniper", "--full"]),
        Cmd::Verify {
            targets: vec![rt("sniper")],
            full: true,
        }
    );
    assert_eq!(
        plain(&["import", "SteamLinuxRuntime_sniper", "--link"]),
        Cmd::Import {
            sources: vec!["SteamLinuxRuntime_sniper".into()],
            link: true,
        }
    );

    for args in [&["remove"][..], &["info"], &["files"], &["import"]] {
        assert_eq!(
            error(args).kind(),
            ErrorKind::MissingRequiredArgument,
            "{args:?}"
        );
    }
    let message = error(&["remove", "Bad Name"]).render().to_string();
    assert!(message.contains("invalid value 'Bad Name'"), "{message}");
    let message = error(&["info", "sniper=x"]).render().to_string();
    assert!(message.contains("is not a build id"), "{message}");
}

#[test]
fn running() {
    assert_eq!(
        plain(&["run", "hollow-knight"]),
        Cmd::Run(Run {
            game: "hollow-knight".into(),
            args: Vec::new(),
            profile: None,
            mode: LaunchMode::Detach,
            debug: false,
        })
    );
    // Everything after `--` is the game's, `-` options included.
    assert_eq!(
        plain(&[
            "run", "-p", "scout", "--wait", "--debug", "hk", "--", "-x", "--wait"
        ]),
        Cmd::Run(Run {
            game: "hk".into(),
            args: strings(&["-x", "--wait"]),
            profile: Some("scout".into()),
            mode: LaunchMode::Wait,
            debug: true,
        })
    );
    assert_eq!(
        plain(&["execute", "-p", "sniper", "./setup.sh", "--", "--silent"]),
        Cmd::Execute(Execute {
            target: "./setup.sh".into(),
            args: strings(&["--silent"]),
            profile: Some("sniper".into()),
            mode: LaunchMode::Detach,
            debug: false,
        })
    );
    assert_eq!(
        plain(&["execute", "glxgears", "--wait"]),
        Cmd::Execute(Execute {
            target: "glxgears".into(),
            args: Vec::new(),
            profile: None,
            mode: LaunchMode::Wait,
            debug: false,
        })
    );
    assert_eq!(
        plain(&["shell"]),
        Cmd::Shell {
            profile: None,
            debug: false
        }
    );
    assert_eq!(
        plain(&["shell", "--profile", "scout", "--debug"]),
        Cmd::Shell {
            profile: Some("scout".into()),
            debug: true
        }
    );
    assert_eq!(
        plain(&["stop", "hk", "sniper", "1234"]),
        Cmd::Stop {
            targets: strings(&["hk", "sniper", "1234"])
        }
    );
    assert_eq!(plain(&["killall"]), Cmd::Killall);

    // A game name never runs a path (D10), and game arguments need `--`.
    let message = error(&["run", "./game"]).render().to_string();
    assert!(message.contains("invalid value './game'"), "{message}");
    assert_eq!(
        error(&["run", "hk", "-x"]).kind(),
        ErrorKind::UnknownArgument
    );
    assert_eq!(error(&["stop"]).kind(), ErrorKind::MissingRequiredArgument);
    assert_eq!(error(&["killall", "hk"]).kind(), ErrorKind::UnknownArgument);
}

#[test]
fn games() {
    assert_eq!(plain(&["list"]), Cmd::List);
    assert_eq!(plain(&["show", "hk"]), Cmd::Show { game: "hk".into() });
    assert_eq!(
        plain(&["check"]),
        Cmd::Check {
            targets: Vec::new()
        }
    );
    assert_eq!(
        plain(&["check", "hk", "scout"]),
        Cmd::Check {
            targets: strings(&["hk", "scout"])
        }
    );
    assert_eq!(
        plain(&["logs", "hk"]),
        Cmd::Logs {
            game: "hk".into(),
            path_only: false
        }
    );
    assert_eq!(
        plain(&["logs", "hk", "--path"]),
        Cmd::Logs {
            game: "hk".into(),
            path_only: true
        }
    );
    assert_eq!(
        plain(&["init", "scout"]),
        Cmd::Init {
            profile: "scout".into()
        }
    );
    assert_eq!(error(&["show"]).kind(), ErrorKind::MissingRequiredArgument);
}

#[test]
fn profiles() {
    assert_eq!(plain(&["profiles", "list"]), Cmd::Profiles(Profiles::List));
    assert_eq!(
        plain(&["profiles", "show", "scout"]),
        Cmd::Profiles(Profiles::Show {
            name: "scout".into()
        })
    );
    assert_eq!(
        plain(&["profiles", "new", "indie", "--runtime", "sniper"]),
        Cmd::Profiles(Profiles::New {
            name: "indie".into(),
            runtime: rt("sniper"),
            mode: SlrMode::Container,
            prefix: None,
        })
    );
    assert_eq!(
        plain(&[
            "profiles",
            "new",
            "host",
            "-r",
            "steamrt4=4.0.1",
            "--mode",
            "overlay",
            "--prefix",
            "~/p",
        ]),
        Cmd::Profiles(Profiles::New {
            name: "host".into(),
            runtime: rt("steamrt4=4.0.1"),
            mode: SlrMode::Overlay,
            prefix: Some("~/p".into()),
        })
    );
    assert_eq!(
        plain(&["profiles", "edit", "indie"]),
        Cmd::Profiles(Profiles::Edit {
            name: "indie".into()
        })
    );
    assert_eq!(
        plain(&["profiles", "edit-games"]),
        Cmd::Profiles(Profiles::EditGames)
    );
    assert_eq!(
        plain(&["profiles", "delete", "a", "b", "--prefix"]),
        Cmd::Profiles(Profiles::Delete {
            names: strings(&["a", "b"]),
            prefix: true,
        })
    );

    assert_eq!(
        error(&["profiles", "new", "indie"]).kind(),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        error(&[
            "profiles", "new", "indie", "-r", "sniper", "--mode", "chroot"
        ])
        .kind(),
        ErrorKind::InvalidValue
    );
    assert_eq!(
        error(&["profiles", "delete"]).kind(),
        ErrorKind::MissingRequiredArgument
    );
    assert_eq!(
        error(&["profiles"]).kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}

#[cfg(feature = "diag")]
#[test]
fn doctor() {
    assert_eq!(
        plain(&["doctor"]),
        Cmd::Doctor {
            runtimes: Vec::new()
        }
    );
    assert_eq!(
        plain(&["doctor", "scout", "sniper"]),
        Cmd::Doctor {
            runtimes: vec![rt("scout"), rt("sniper")]
        }
    );
}

#[cfg(feature = "json")]
#[test]
fn json_only_where_the_plan_lists_it() {
    #[cfg_attr(not(feature = "download"), allow(unused_mut))]
    let mut with_json = vec![
        &["info", "sniper"][..],
        &["runtimes"],
        &["status"],
        &["list"],
        &["show", "hk"],
        &["profiles", "list"],
        &["profiles", "show", "p"],
        &["provide-umu", "sniper"],
        &["provide-umu", "--appid", "1628350"],
    ];
    #[cfg(feature = "download")]
    with_json.extend([&["builds", "sniper"][..], &["outdated"]]);
    for args in with_json {
        let mut args = args.to_vec();
        args.push("--json");
        assert!(cmd(&args).1, "{args:?}");
    }
    #[cfg_attr(not(feature = "download"), allow(unused_mut))]
    let mut without = vec![
        &["remove", "x"][..],
        &["verify"],
        &["files", "x"],
        &["check"],
        &["killall"],
        &["profiles", "edit-games"],
    ];
    #[cfg(feature = "download")]
    without.extend([&["install", "x"][..], &["sync"], &["search"], &["clean"]]);
    for args in without {
        let mut args = args.to_vec();
        args.push("--json");
        assert_eq!(error(&args).kind(), ErrorKind::UnknownArgument, "{args:?}");
    }
}

#[test]
fn global_options_go_anywhere() {
    let options = parse(&[
        "--color", "never", "runtimes", "-y", "-vv", "--config", "c.toml",
    ])
    .unwrap();
    assert_eq!(options.color, Some(ColorChoice::Never));
    assert!(options.yes);
    assert_eq!(options.verbose, 2);
    assert_eq!(options.config, Some("c.toml".into()));
    assert!(!options.quiet && !options.json);

    assert_eq!(
        error(&["--color", "sometimes", "runtimes"]).kind(),
        ErrorKind::InvalidValue
    );
    assert_eq!(
        error(&["-S"]).kind(),
        ErrorKind::UnknownArgument,
        "no pacman flags"
    );
    assert_eq!(error(&["frobnicate"]).kind(), ErrorKind::InvalidSubcommand);
}

#[test]
fn the_definition_is_consistent() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
}

#[test]
fn provide_umu_takes_a_runtime_or_an_app_id() {
    assert_eq!(
        cmd(&["provide-umu", "sniper=3.0.1"]).0,
        Cmd::ProvideUmu {
            runtime: Some("sniper=3.0.1".parse().unwrap()),
            app_id: None,
        }
    );
    assert_eq!(
        cmd(&["provide-umu", "--appid", "4183110"]).0,
        Cmd::ProvideUmu {
            runtime: None,
            app_id: Some(4_183_110),
        }
    );
    for args in [
        &["provide-umu"][..],
        &["provide-umu", "sniper", "--appid", "1"],
        &["provide-umu", "--appid", "x"],
    ] {
        assert!(parse(args).is_err(), "{args:?}");
    }
}
