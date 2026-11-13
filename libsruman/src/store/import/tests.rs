use std::fs;

use super::*;
use crate::store::tests::fake_container;

fn catalog() -> Catalog {
    Catalog::builtin()
}

#[test]
fn detects_container_runtimes() {
    let tmp = tempfile::tempdir().unwrap();
    fake_container(tmp.path(), "sniper", "3.0.20261021.266025");
    let found = detect(tmp.path(), &catalog()).unwrap();
    assert_eq!(found.name, "sniper");
    assert_eq!(found.build.as_str(), "3.0.20261021.266025");
    assert_eq!(found.entry, Entry::Container);
}

#[test]
fn platform_dir_suffix_is_the_fallback_build() {
    let tmp = tempfile::tempdir().unwrap();
    fake_container(tmp.path(), "steamrt4", "4.0.1");
    fs::remove_file(tmp.path().join("VERSIONS.txt")).unwrap();
    let found = detect(tmp.path(), &catalog()).unwrap();
    assert_eq!(
        (found.name.as_str(), found.build.as_str()),
        ("steamrt4", "4.0.1")
    );
}

#[test]
fn detects_scout_and_scout_ldlp() {
    let tmp = tempfile::tempdir().unwrap();
    let scout = tmp.path().join("SteamLinuxRuntime");
    fs::create_dir_all(scout.join("steam-runtime")).unwrap();
    fs::write(scout.join("scout-on-soldier-entry-point-v2"), "").unwrap();
    fs::write(
        scout.join("VERSIONS.txt"),
        "depot\t1.0.20261021.266054\t\t\t#\n",
    )
    .unwrap();
    let found = detect(&scout, &catalog()).unwrap();
    assert_eq!(
        (found.name.as_str(), found.entry),
        ("scout", Entry::ScoutOnSoldier)
    );

    let ldlp = scout.join("steam-runtime");
    fs::write(ldlp.join("run.sh"), "").unwrap();
    fs::write(
        ldlp.join("version.txt"),
        "steam-runtime_1.0.20261021.266054\n",
    )
    .unwrap();
    let found = detect(&ldlp, &catalog()).unwrap();
    assert_eq!(
        (found.name.as_str(), found.build.as_str()),
        ("scout-ldlp", "1.0.20261021.266054")
    );

    fs::write(ldlp.join("version.txt"), "something else\n").unwrap();
    assert!(matches!(
        detect(&ldlp, &catalog()),
        Err(ImportError::NoBuildId { .. })
    ));
}

#[test]
fn rejects_non_runtimes_and_unknown_names() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(
        detect(tmp.path(), &catalog()),
        Err(ImportError::NotARuntime(_))
    ));
    fake_container(tmp.path(), "heavy", "1.0");
    let err = detect(tmp.path(), &catalog()).unwrap_err();
    assert!(
        err.to_string()
            .ends_with("holds `heavy`, which isn't in the runtime catalog"),
        "{err}"
    );
}

#[test]
fn import_copies_without_var_and_refuses_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("SteamLinuxRuntime_sniper");
    fs::create_dir(&src).unwrap();
    fake_container(&src, "sniper", "3.0.7");
    fs::create_dir_all(src.join("var/tmp-XYZ")).unwrap();
    std::os::unix::fs::symlink("run", src.join("run-in-sniper")).unwrap();
    let store = Store::open(tmp.path().join("store"));
    let lock = store.lock().unwrap();

    let installed = import(
        &store,
        &lock,
        &catalog(),
        &src,
        Some("sniper"),
        ImportMode::Copy,
    )
    .unwrap();
    assert_eq!(installed.build.as_str(), "3.0.7");
    assert!(installed.is_current);
    assert!(!installed.dir.is_symlink());
    assert!(installed.dir.join("run").is_file());
    assert!(!installed.dir.join("var").exists());
    assert_eq!(
        fs::read_link(installed.dir.join("run-in-sniper")).unwrap(),
        Path::new("run")
    );
    assert!(matches!(
        installed.meta.unwrap().source,
        Source::Import { .. }
    ));
    assert_eq!(
        store
            .verify(
                &store.resolve(&installed_ref()).unwrap(),
                crate::mtree::Depth::Content
            )
            .unwrap()
            .unwrap()
            .problems,
        []
    );

    let again = import(&store, &lock, &catalog(), &src, None, ImportMode::Copy).unwrap_err();
    assert!(
        matches!(
            again,
            ImportError::Store(StoreError::AlreadyInstalled { .. })
        ),
        "{again}"
    );
}

fn installed_ref() -> crate::version::RuntimeRef {
    "sniper".parse().unwrap()
}

#[test]
fn import_links_in_place_and_checks_the_expected_name() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("SteamLinuxRuntime_soldier");
    fs::create_dir(&src).unwrap();
    fake_container(&src, "soldier", "2.0.3");
    let store = Store::open(tmp.path().join("store"));
    let lock = store.lock().unwrap();

    let err = import(
        &store,
        &lock,
        &catalog(),
        &src,
        Some("sniper"),
        ImportMode::Link,
    )
    .unwrap_err();
    assert!(
        err.to_string().ends_with("holds `soldier`, not `sniper`"),
        "{err}"
    );

    let linked = import(&store, &lock, &catalog(), &src, None, ImportMode::Link).unwrap();
    assert!(linked.is_linked());
    assert_eq!(
        fs::canonicalize(&linked.dir).unwrap(),
        fs::canonicalize(&src).unwrap()
    );
}

/// Steam's real installs, when present.
#[test]
#[ignore = "needs Steam's SteamLinuxRuntime_soldier and SteamLinuxRuntime installed"]
fn detects_real_steam_installs() {
    let home = std::env::var_os("HOME").unwrap();
    let common = Path::new(&home).join(".local/share/Steam/steamapps/common");
    let soldier = detect(&common.join("SteamLinuxRuntime_soldier"), &catalog()).unwrap();
    assert_eq!(soldier.name, "soldier");
    let scout = detect(&common.join("SteamLinuxRuntime"), &catalog()).unwrap();
    assert_eq!(scout.name, "scout");
    let ldlp = detect(&common.join("SteamLinuxRuntime/steam-runtime"), &catalog()).unwrap();
    assert_eq!(ldlp.name, "scout-ldlp");
    assert_eq!(ldlp.build, scout.build);
}
