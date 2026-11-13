use super::*;

fn names(defs: &[&RuntimeDef]) -> Vec<String> {
    defs.iter().map(|def| def.name.clone()).collect()
}

#[test]
fn builtin_matches_valves_server() {
    let catalog = Catalog::builtin();
    assert_eq!(
        catalog.names().collect::<Vec<_>>(),
        ["scout", "scout-ldlp", "sniper", "soldier", "steamrt4"]
    );
    let sniper = catalog.get("sniper").unwrap();
    assert_eq!(
        sniper.images_url(),
        "https://repo.steampowered.com/steamrt3/images"
    );
    assert_eq!(
        sniper.archive(Arch::X86_64),
        Some("SteamLinuxRuntime_sniper.tar.xz")
    );
    assert_eq!(sniper.archive(Arch::Arm64), None);
    assert_eq!(sniper.entry, Entry::Container);

    let scout = catalog.get("scout").unwrap();
    assert_eq!(scout.entry, Entry::ScoutOnSoldier);
    assert_eq!(scout.depends, ["soldier"]);
    assert_eq!(catalog.get("scout-ldlp").unwrap().entry, Entry::Ldlp);
    assert_eq!(
        catalog.get("steamrt4").unwrap().archive(Arch::Arm64),
        Some("SteamLinuxRuntime_4-arm64.tar.xz")
    );
}

#[test]
fn entry_markers() {
    assert_eq!(Entry::Container.marker(), "run");
    assert_eq!(
        Entry::ScoutOnSoldier.marker(),
        "scout-on-soldier-entry-point-v2"
    );
    assert_eq!(Entry::Ldlp.marker(), "run.sh");
}

#[test]
fn install_order_puts_dependencies_first_once() {
    let catalog = Catalog::builtin();
    assert_eq!(
        names(&catalog.install_order(["scout"]).unwrap()),
        ["soldier", "scout"]
    );
    assert_eq!(
        names(
            &catalog
                .install_order(["scout", "soldier", "sniper", "scout"])
                .unwrap()
        ),
        ["soldier", "scout", "sniper"]
    );
    assert!(
        catalog
            .install_order::<[&str; 0], str>([])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_runtime_and_missing_dependency() {
    let catalog = Catalog::builtin();
    assert_eq!(
        catalog.install_order(["snipper"]).unwrap_err(),
        CatalogError::Unknown {
            name: "snipper".into(),
            required_by: None
        }
    );

    let broken = catalog
        .with_overrides(&[RuntimeOverride {
            name: "sniper".into(),
            depends: Some(vec!["gone".into()]),
            ..Default::default()
        }])
        .unwrap();
    let err = broken.install_order(["sniper"]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown runtime `gone` (required by `sniper`)"
    );
}

#[test]
fn cycles_are_reported_with_their_path() {
    let over = |name: &str, dep: &str| RuntimeOverride {
        name: name.into(),
        depends: Some(vec![dep.into()]),
        ..Default::default()
    };
    let catalog = Catalog::builtin()
        .with_overrides(&[over("soldier", "sniper"), over("sniper", "scout")])
        .unwrap();
    let err = catalog.install_order(["sniper"]).unwrap_err();
    assert_eq!(
        err,
        CatalogError::Cycle(vec![
            "sniper".into(),
            "scout".into(),
            "soldier".into(),
            "sniper".into()
        ])
    );
}

#[test]
fn override_merges_into_builtin() {
    let catalog = Catalog::builtin()
        .with_overrides(&[RuntimeOverride {
            name: "sniper".into(),
            base_url: Some("https://mirror.example/steam/".into()),
            pointers: Some([("beta".to_owned(), vec!["my-beta".to_owned()])].into()),
            ..Default::default()
        }])
        .unwrap();
    let sniper = catalog.get("sniper").unwrap();
    assert_eq!(
        sniper.images_url(),
        "https://mirror.example/steam/steamrt3/images"
    );
    assert_eq!(
        sniper.archive(Arch::X86_64),
        Some("SteamLinuxRuntime_sniper.tar.xz")
    );
    assert_eq!(sniper.pointer_files(Channel::Beta), ["my-beta"]);
    assert_eq!(
        sniper.pointer_files(Channel::Stable),
        ["latest-public-stable"]
    );
}

#[test]
fn override_adds_a_new_runtime_from_toml() {
    #[derive(serde::Deserialize)]
    struct Config {
        runtime: Vec<RuntimeOverride>,
    }
    let config: Config = toml::from_str(
        r#"
        [[runtime]]
        name = "steamrt5"
        server_dir = "steamrt5"
        archives = { x86_64 = "SteamLinuxRuntime_5.tar.xz" }
        entry = "container"
        "#,
    )
    .unwrap();
    let catalog = Catalog::builtin().with_overrides(&config.runtime).unwrap();
    let def = catalog.get("steamrt5").unwrap();
    assert_eq!(def.entry, Entry::Container);
    assert_eq!(
        def.archive(Arch::X86_64),
        Some("SteamLinuxRuntime_5.tar.xz")
    );
    assert_eq!(def.base_url, DEFAULT_BASE_URL);
}

#[test]
fn invalid_overrides_are_all_reported() {
    let errors = Catalog::builtin()
        .with_overrides(&[
            RuntimeOverride {
                name: "new".into(),
                ..Default::default()
            },
            RuntimeOverride {
                name: "sniper".into(),
                base_url: Some("http://insecure.example".into()),
                ..Default::default()
            },
            RuntimeOverride {
                name: "soldier".into(),
                server_dir: Some("../etc".into()),
                ..Default::default()
            },
            RuntimeOverride {
                name: "steamrt4".into(),
                archives: Some([(Arch::X86_64, "../x.tar.xz".to_owned())].into()),
                ..Default::default()
            },
            RuntimeOverride {
                name: "scout".into(),
                pointers: Some([("nightly".to_owned(), vec![])].into()),
                ..Default::default()
            },
            RuntimeOverride {
                name: "-bad".into(),
                ..Default::default()
            },
        ])
        .unwrap_err();
    let text: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(text.len(), 6, "{text:#?}");
    assert!(text[0].contains("needs `server_dir`, `archives` and `entry`"));
    assert!(text[1].contains("https://"));
    assert!(text[2].contains("plain directory name"));
    assert!(text[3].contains("plain file names"));
    assert!(text[4].contains("`pointers.nightly` is not a channel"));
    assert!(text[5].starts_with("[[runtime]] `-bad`"));
}

#[test]
fn dependents_lists_direct_users() {
    let catalog = Catalog::builtin();
    assert_eq!(names(&catalog.dependents("soldier")), ["scout"]);
    assert!(catalog.dependents("sniper").is_empty());
}

#[test]
fn current_arch_is_known_on_ci_targets() {
    if cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        assert!(Arch::current().is_some());
    }
}
