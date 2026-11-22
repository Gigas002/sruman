use super::*;

fn ctx<'a>(default_profile: Option<&'a str>) -> Context<'a> {
    Context {
        cwd: Path::new("/work"),
        home: Path::new("/home/u"),
        default_profile,
    }
}

fn execute(target: &str, profile: Option<&str>) -> Command {
    Command::Execute(Execute {
        target: target.into(),
        args: vec!["-x".into()],
        profile: profile.map(str::to_owned),
        mode: LaunchMode::Detach,
        debug: false,
    })
}

#[test]
fn execute_paths_become_absolute_and_programs_stay_bare() {
    for (target, expected) in [
        ("./setup.sh", "/work/./setup.sh"),
        ("bin/game", "/work/bin/game"),
        ("/opt/game", "/opt/game"),
        ("~/Games/x", "/home/u/Games/x"),
        ("glxgears", "glxgears"),
    ] {
        let resolved = execute(target, Some("p")).resolve(&ctx(None)).unwrap();
        let Command::Execute(execute) = resolved else {
            panic!("{resolved:?}")
        };
        assert_eq!(execute.target, PathBuf::from(expected), "{target}");
        assert_eq!(execute.profile.as_deref(), Some("p"));
        assert_eq!(execute.args, ["-x"]);
    }
}

#[test]
fn execute_and_shell_fall_back_to_default_profile() {
    let resolved = execute("glxgears", None)
        .resolve(&ctx(Some("sniper")))
        .unwrap();
    assert!(matches!(&resolved, Command::Execute(e) if e.profile.as_deref() == Some("sniper")));
    // -p wins.
    let resolved = execute("glxgears", Some("mine"))
        .resolve(&ctx(Some("sniper")))
        .unwrap();
    assert!(matches!(&resolved, Command::Execute(e) if e.profile.as_deref() == Some("mine")));

    let shell = Command::Shell {
        profile: None,
        debug: true,
    };
    assert_eq!(
        shell.clone().resolve(&ctx(Some("sniper"))).unwrap(),
        Command::Shell {
            profile: Some("sniper".into()),
            debug: true
        }
    );
    assert_eq!(
        shell.resolve(&ctx(None)).unwrap_err().to_string(),
        "shell needs a profile: -p <profile>, or default_profile in config.toml"
    );
    assert_eq!(
        execute("x", None)
            .resolve(&ctx(None))
            .unwrap_err()
            .to_string(),
        "execute needs a profile: -p <profile>, or default_profile in config.toml"
    );
}

#[test]
fn import_sources_and_new_prefixes_are_relative_to_the_cwd() {
    let import = Command::Import {
        sources: vec!["rt.tar.xz".into(), "/abs/dir".into(), "~/SLR".into()],
        link: true,
    };
    assert_eq!(
        import.resolve(&ctx(None)).unwrap(),
        Command::Import {
            sources: vec![
                "/work/rt.tar.xz".into(),
                "/abs/dir".into(),
                "/home/u/SLR".into()
            ],
            link: true,
        }
    );
    let new = |prefix: Option<&str>| {
        Command::Profiles(Profiles::New {
            name: "p".into(),
            runtime: "sniper".parse().unwrap(),
            mode: SlrMode::Overlay,
            prefix: prefix.map(PathBuf::from),
        })
    };
    assert_eq!(
        new(Some("pfx")).resolve(&ctx(None)).unwrap(),
        new(Some("/work/pfx"))
    );
    assert_eq!(new(None).resolve(&ctx(None)).unwrap(), new(None));
}

#[test]
fn names_and_launches() {
    assert_eq!(Command::Killall.name(), "killall");
    assert_eq!(
        Command::Profiles(Profiles::EditGames).name(),
        "profiles edit-games"
    );
    assert!(execute("x", None).is_launch());
    assert!(!Command::Status.is_launch());
    // Commands without paths or profiles pass through unchanged.
    assert_eq!(
        Command::Status.resolve(&ctx(None)).unwrap(),
        Command::Status
    );
}
