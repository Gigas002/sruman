//! Every row of the CLI tables in PLAN.md, from argv to [`Operation`].

use std::path::{Path, PathBuf};

use libsruman::version::RuntimeRef;

use super::*;
use crate::settings::{
    CliOptions,
    operation::{self, Context, Exec, Games, Operation, Profiles, Query, Run},
};

fn parse(args: &[&str]) -> Result<CliOptions, String> {
    Cli::try_parse_args(std::iter::once("sruman").chain(args.iter().copied()))
        .map(Into::into)
        .map_err(|err| err.to_string())
}

fn resolve_with(args: &[&str], default_profile: Option<&str>) -> Result<Operation, String> {
    let options = parse(args)?;
    let ctx = Context {
        cwd: Path::new("/work"),
        home: Path::new("/home/u"),
        default_profile,
    };
    let json = options.invocation.json;
    let operation = operation::resolve(options.invocation, &ctx).map_err(|err| err.0)?;
    operation::check_json(&operation, json).map_err(|err| err.0)?;
    Ok(operation)
}

fn resolve(args: &[&str]) -> Operation {
    resolve_with(args, None).unwrap_or_else(|err| panic!("{args:?}: {err}"))
}

fn error(args: &[&str]) -> String {
    match resolve_with(args, None) {
        Ok(operation) => panic!("{args:?} resolved to {operation:?}"),
        Err(err) => err,
    }
}

fn rt(text: &str) -> RuntimeRef {
    text.parse().unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[cfg(feature = "download")]
mod sync {
    use super::*;
    use crate::settings::operation::{Sync, SyncAction};

    fn sync(refresh: u8, action: SyncAction) -> Operation {
        Operation::Sync(Sync { refresh, action })
    }

    fn upgrade(targets: &[&str], allow_downgrade: bool, download_only: bool) -> SyncAction {
        SyncAction::Upgrade {
            targets: strings(targets),
            allow_downgrade,
            download_only,
        }
    }

    #[test]
    fn bare_sruman_is_syu() {
        assert_eq!(resolve(&[]), sync(1, upgrade(&[], false, false)));
        assert_eq!(resolve(&["-Syu"]), resolve(&[]));
    }

    #[test]
    fn install() {
        let install = |targets: &[&str], needed, download_only| SyncAction::Install {
            targets: targets.iter().map(|t| rt(t)).collect(),
            needed,
            download_only,
        };
        assert_eq!(
            resolve(&["-S", "sniper", "scout=1.0.20261021.266054"]),
            sync(
                0,
                install(&["sniper", "scout=1.0.20261021.266054"], false, false)
            )
        );
        assert_eq!(
            resolve(&["-S", "--needed", "sniper"]),
            sync(0, install(&["sniper"], true, false))
        );
        assert_eq!(
            resolve(&["-Sw", "sniper"]),
            sync(0, install(&["sniper"], false, true))
        );
        assert_eq!(
            resolve(&["-Sy", "sniper"]),
            sync(1, install(&["sniper"], false, false))
        );
    }

    #[test]
    fn options_after_targets() {
        assert_eq!(
            resolve(&["-S", "sniper", "--needed"]),
            resolve(&["-S", "--needed", "sniper"])
        );
        assert_eq!(
            resolve(&["-S", "sniper", "-w"]),
            resolve(&["-Sw", "sniper"])
        );
    }

    #[test]
    fn refresh() {
        assert_eq!(resolve(&["-Sy"]), sync(1, SyncAction::RefreshOnly));
        assert_eq!(resolve(&["-Syy"]), sync(2, SyncAction::RefreshOnly));
        assert_eq!(resolve(&["-Syyy"]), sync(2, SyncAction::RefreshOnly));
    }

    #[test]
    fn upgrades() {
        assert_eq!(resolve(&["-Su"]), sync(0, upgrade(&[], false, false)));
        assert_eq!(
            resolve(&["-Syu", "sniper"]),
            sync(1, upgrade(&["sniper"], false, false))
        );
        assert_eq!(resolve(&["-Syuu"]), sync(1, upgrade(&[], true, false)));
        assert_eq!(resolve(&["-Suw"]), sync(0, upgrade(&[], false, true)));
    }

    #[test]
    fn search() {
        assert_eq!(
            resolve(&["-Ss"]),
            sync(
                0,
                SyncAction::Search {
                    terms: vec![],
                    packages: false
                }
            )
        );
        assert_eq!(
            resolve(&["-Ss", "--pkg", "mesa"]),
            sync(
                0,
                SyncAction::Search {
                    terms: strings(&["mesa"]),
                    packages: true
                }
            )
        );
    }

    #[test]
    fn info_list_clean() {
        assert_eq!(
            resolve(&["-Si", "sniper"]),
            sync(
                0,
                SyncAction::Info {
                    targets: vec![rt("sniper")],
                    packages: false
                }
            )
        );
        assert_eq!(
            resolve(&["-Sii", "sniper=3.0.1"]),
            sync(
                0,
                SyncAction::Info {
                    targets: vec![rt("sniper=3.0.1")],
                    packages: true
                }
            )
        );
        assert_eq!(
            resolve(&["-Sl"]),
            sync(0, SyncAction::List { targets: vec![] })
        );
        assert_eq!(
            resolve(&["-Sl", "scout"]),
            sync(
                0,
                SyncAction::List {
                    targets: strings(&["scout"])
                }
            )
        );
        assert_eq!(resolve(&["-Sc"]), sync(0, SyncAction::Clean { all: false }));
        assert_eq!(resolve(&["-Scc"]), sync(0, SyncAction::Clean { all: true }));
    }

    #[test]
    fn sync_errors() {
        assert_eq!(error(&["-S"]), "no targets specified (use -h for help)");
        assert_eq!(error(&["-Sk"]), "invalid option '-k' for -S");
        assert_eq!(
            error(&["-Ssi"]),
            "invalid option: '-s' and '-i' may not be used together"
        );
        assert_eq!(
            error(&["-Ss", "--needed"]),
            "invalid option '--needed' for -Ss"
        );
        assert_eq!(
            error(&["-Su", "--needed"]),
            "invalid option '--needed' for -Su"
        );
        assert_eq!(
            error(&["-S", "--pkg", "x"]),
            "invalid option '--pkg' for -S"
        );
        assert_eq!(error(&["-Sc", "sniper"]), "-Sc takes no targets");
        assert_eq!(
            error(&["-Su", "sniper=3.0.1"]),
            "-Su takes runtime names, not builds: 'sniper=3.0.1'"
        );
        assert!(error(&["-S", "sniper=nope"]).starts_with("invalid runtime 'sniper=nope'"));
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_on_sl_only() {
        resolve(&["-Sl", "--json"]);
        assert_eq!(error(&["-Ss", "--json"]), "invalid option '--json' for -Ss");
    }

    #[test]
    fn query_upgradable() {
        assert_eq!(
            resolve(&["-Qu"]),
            Operation::Query(Query::Upgradable { targets: vec![] })
        );
    }
}

#[test]
fn query() {
    assert_eq!(
        resolve(&["-Q"]),
        Operation::Query(Query::List {
            targets: vec![],
            quiet: false
        })
    );
    assert_eq!(
        resolve(&["-Qq", "sniper"]),
        Operation::Query(Query::List {
            targets: vec![rt("sniper")],
            quiet: true
        })
    );
    assert_eq!(
        resolve(&["-Qi", "sniper"]),
        Operation::Query(Query::Info {
            targets: vec![rt("sniper")]
        })
    );
    assert_eq!(
        resolve(&["-Qk"]),
        Operation::Query(Query::Check {
            targets: vec![],
            full: false
        })
    );
    assert_eq!(
        resolve(&["-Qkk", "sniper"]),
        Operation::Query(Query::Check {
            targets: vec![rt("sniper")],
            full: true
        })
    );
    assert_eq!(
        resolve(&["-Ql", "sniper"]),
        Operation::Query(Query::Files {
            targets: vec![rt("sniper")]
        })
    );
    assert_eq!(error(&["-Ql"]), "no targets specified (use -h for help)");
    assert_eq!(
        error(&["-Qik"]),
        "invalid option: '-i' and '-k' may not be used together"
    );
    assert_eq!(error(&["-Qy"]), "invalid option '-y' for -Q");
}

#[test]
fn remove() {
    assert_eq!(
        resolve(&["-R", "sniper=3.0.1"]),
        Operation::Remove {
            targets: vec![rt("sniper=3.0.1")],
            nodeps: 0
        }
    );
    assert_eq!(
        resolve(&["-Rdd", "sniper"]),
        Operation::Remove {
            targets: vec![rt("sniper")],
            nodeps: 2
        }
    );
    assert_eq!(error(&["-R"]), "no targets specified (use -h for help)");
}

#[test]
fn local_install() {
    assert_eq!(
        resolve(&["-U", "sniper.tar.xz", "/abs/SteamLinuxRuntime_sniper"]),
        Operation::LocalInstall {
            sources: vec![
                PathBuf::from("/work/sniper.tar.xz"),
                PathBuf::from("/abs/SteamLinuxRuntime_sniper")
            ],
            link: false
        }
    );
    assert_eq!(
        resolve(&["-U", "--link", "~/steamrt3"]),
        Operation::LocalInstall {
            sources: vec![PathBuf::from("/home/u/steamrt3")],
            link: true
        }
    );
}

fn run(target: &str, args: &[&str], profile: Option<&str>, wait: bool, debug: bool) -> Operation {
    Operation::Exec(Exec::Run(Run {
        target: target.to_owned(),
        args: strings(args),
        profile: profile.map(str::to_owned),
        wait,
        debug,
    }))
}

#[test]
fn bare_targets_run() {
    assert_eq!(
        resolve(&["hollow-knight"]),
        run("hollow-knight", &[], None, false, false)
    );
    assert_eq!(
        resolve(&["-p", "scout", "./hollow_knight.x86_64"]),
        run("./hollow_knight.x86_64", &[], Some("scout"), false, false)
    );
    assert_eq!(
        resolve(&["--wait", "--debug", "game", "-force-vulkan", "--wait", "-S"]),
        run("game", &["-force-vulkan", "--wait", "-S"], None, true, true)
    );
}

#[test]
fn exec() {
    assert_eq!(
        resolve(&["-X", "-p", "sniper", "./game", "-v"]),
        run("./game", &["-v"], Some("sniper"), false, false)
    );
    assert_eq!(resolve(&["-X", "game"]), resolve(&["game"]));
    assert_eq!(
        resolve(&["-X", "-p", "sniper", "--shell"]),
        Operation::Exec(Exec::Shell {
            profile: "sniper".into(),
            debug: false
        })
    );
    assert_eq!(resolve(&["-Xs"]), Operation::Exec(Exec::Status));
    assert_eq!(
        resolve(&["-Xk", "hollow-knight", "1234"]),
        Operation::Exec(Exec::Stop {
            targets: strings(&["hollow-knight", "1234"])
        })
    );
    assert_eq!(resolve(&["-Xkk"]), Operation::Exec(Exec::StopAll));
}

#[test]
fn shell_uses_the_default_profile() {
    assert_eq!(
        resolve_with(&["-X", "--shell", "--debug"], Some("soldier")).unwrap(),
        Operation::Exec(Exec::Shell {
            profile: "soldier".into(),
            debug: true
        })
    );
    assert_eq!(
        error(&["--shell"]),
        "--shell needs a profile: -p <profile>, or default_profile in config.toml"
    );
}

#[test]
fn exec_errors() {
    assert_eq!(error(&["-X"]), "no targets specified (use -h for help)");
    assert_eq!(error(&["-Xs", "--wait"]), "invalid option '--wait' for -Xs");
    assert_eq!(error(&["-Xk"]), "no targets specified (use -h for help)");
    assert_eq!(error(&["-Xkk", "game"]), "-Xkk takes no targets");
    assert_eq!(
        error(&["-X", "--shell", "-p", "sniper", "game"]),
        "--shell takes no targets"
    );
    assert_eq!(
        error(&["-X", "--shell", "-p", "sniper", "--wait"]),
        "invalid option '--wait' for --shell"
    );
    assert!(error(&["-p", "../x", "game"]).starts_with("invalid profile:"));
}

#[test]
fn games() {
    assert_eq!(
        resolve(&["-G"]),
        Operation::Games(Games::List { targets: vec![] })
    );
    assert_eq!(
        resolve(&["-Gi", "hollow-knight"]),
        Operation::Games(Games::Info {
            targets: strings(&["hollow-knight"])
        })
    );
    assert_eq!(
        resolve(&["-Gk"]),
        Operation::Games(Games::Check { targets: vec![] })
    );
    assert_eq!(resolve(&["-Ge"]), Operation::Games(Games::Edit));
    assert_eq!(
        resolve(&["-Gl", "hk", "--path"]),
        Operation::Games(Games::Log {
            game: "hk".into(),
            path_only: true
        })
    );
    assert_eq!(error(&["-Gl"]), "-Gl takes exactly one game");
    assert_eq!(
        error(&["-Gi", "--path", "hk"]),
        "invalid option '--path' for -Gi"
    );
    assert_eq!(error(&["-Ge", "hk"]), "-Ge takes no targets");
}

#[test]
fn profiles() {
    assert_eq!(
        resolve(&["-P"]),
        Operation::Profiles(Profiles::List { targets: vec![] })
    );
    assert_eq!(
        resolve(&["-Pi", "sniper"]),
        Operation::Profiles(Profiles::Info {
            targets: strings(&["sniper"])
        })
    );
    assert_eq!(
        resolve(&["-Pk"]),
        Operation::Profiles(Profiles::Check { targets: vec![] })
    );
    assert_eq!(
        resolve(&["-Pn", "hk", "-r", "scout"]),
        Operation::Profiles(Profiles::New {
            name: "hk".into(),
            runtime: rt("scout"),
            prefix: None
        })
    );
    assert_eq!(
        resolve(&["-Pn", "hk", "-r", "scout", "--prefix", "pfx"]),
        Operation::Profiles(Profiles::New {
            name: "hk".into(),
            runtime: rt("scout"),
            prefix: Some(PathBuf::from("/work/pfx"))
        })
    );
    assert_eq!(
        resolve(&["-Pe", "hk"]),
        Operation::Profiles(Profiles::Edit { name: "hk".into() })
    );
    assert_eq!(
        resolve(&["-Pd", "hk"]),
        Operation::Profiles(Profiles::Delete {
            targets: strings(&["hk"]),
            prefix: false
        })
    );
    assert_eq!(
        resolve(&["-Pdd", "hk"]),
        Operation::Profiles(Profiles::Delete {
            targets: strings(&["hk"]),
            prefix: true
        })
    );
}

#[test]
fn profile_errors() {
    assert_eq!(
        error(&["-Pn", "hk"]),
        "-Pn needs a runtime: sruman -Pn hk -r <runtime>"
    );
    assert_eq!(
        error(&["-Pn", "-r", "scout"]),
        "-Pn takes exactly one profile"
    );
    assert_eq!(
        error(&["-Pi", "-r", "scout"]),
        "invalid option '-r' for -Pi"
    );
    assert_eq!(
        error(&["-Pd", "--prefix", "/p", "hk"]),
        "invalid option '--prefix' for -Pd"
    );
    assert_eq!(error(&["-Pe"]), "-Pe takes exactly one profile");
    assert_eq!(error(&["-Pd"]), "no targets specified (use -h for help)");
    assert_eq!(
        error(&["-Pne", "hk"]),
        "invalid option: '-e' and '-n' may not be used together"
    );
    assert!(error(&["-Pi", "a/b"]).starts_with("invalid profile:"));
}

#[cfg(feature = "diag")]
#[test]
fn doctor() {
    assert_eq!(resolve(&["-D"]), Operation::Doctor { runtimes: vec![] });
    assert_eq!(
        resolve(&["-D", "scout"]),
        Operation::Doctor {
            runtimes: vec![rt("scout")]
        }
    );
    assert_eq!(error(&["-Dk"]), "invalid option '-k' for -D");
}

#[test]
fn one_operation_at_a_time() {
    assert_eq!(
        error(&["-QR", "sniper"]),
        "only one operation may be used at a time"
    );
}

#[test]
fn modifiers_need_an_operation() {
    assert_eq!(
        error(&["-k"]),
        "invalid option '-k' without an operation (use -h for help)"
    );
    #[cfg(not(feature = "download"))]
    assert_eq!(error(&[]), "no operation specified (use -h for help)");
}

#[cfg(feature = "json")]
#[test]
fn json_where_supported() {
    for args in [
        &["-Q", "--json"][..],
        &["-Qi", "--json"],
        &["-P", "--json"],
        &["-Pi", "--json"],
        &["-G", "--json"],
        &["-Gi", "hk", "--json"],
        &["-Xs", "--json"],
    ] {
        resolve(args);
    }
    assert_eq!(error(&["-Qk", "--json"]), "invalid option '--json' for -Qk");
    assert_eq!(error(&["--json", "game"]), "invalid option '--json' for -X");
}

#[test]
fn globals() {
    let options = parse(&[
        "--noconfirm",
        "--color",
        "never",
        "-vv",
        "--config",
        "c.toml",
        "-Q",
    ])
    .unwrap();
    assert!(options.noconfirm);
    assert_eq!(options.color, Some(ColorChoice::Never));
    assert_eq!(options.verbose, 2);
    assert_eq!(options.config, Some(PathBuf::from("c.toml")));
}

#[test]
fn clap_rejects_unknown_flags() {
    assert!(parse(&["-Qz"]).is_err());
    assert!(parse(&["--color", "rainbow", "-Q"]).is_err());
}
