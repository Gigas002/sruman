use std::{
    io::Write,
    process::Command,
    sync::{PoisonError, RwLock},
};

use super::*;

/// A spawned child shares every open fd between its fork and exec, which
/// keeps the parent's flocks held after the parent drops them. Tests that
/// check a lock is released take this for reading; tests that spawn take
/// it for writing.
static SPAWN: RwLock<()> = RwLock::new(());

/// A minimal container build like Valve's `SteamLinuxRuntime_<name>`: `run`,
/// `VERSIONS.txt`, `<name>_platform_<build>/files/` and `mtree.txt.gz`.
pub(crate) fn fake_container(dir: &Path, name: &str, build: &str) {
    fs::create_dir_all(dir.join(format!("{name}_platform_{build}/files"))).unwrap();
    fs::write(dir.join("run"), "#!/bin/sh\nexec \"$@\"\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        dir.join("VERSIONS.txt"),
        format!("#Name\tVersion\nruntime\t1\ndepot\t{build}\t\t\t# Overall version number\n"),
    )
    .unwrap();
    let run_sha = mtree::sha256_file(&dir.join("run")).unwrap();
    let text = format!(
        "#mtree\n. type=dir\n./run type=file mode=755 size={} sha256={run_sha}\n./var type=dir ignore optional\n",
        fs::metadata(dir.join("run")).unwrap().len()
    );
    let mut gz = flate2::write::GzEncoder::new(
        File::create(dir.join("mtree.txt.gz")).unwrap(),
        flate2::Compression::fast(),
    );
    gz.write_all(text.as_bytes()).unwrap();
    gz.finish().unwrap();
}

fn id(text: &str) -> BuildId {
    text.parse().unwrap()
}

fn rref(text: &str) -> RuntimeRef {
    text.parse().unwrap()
}

/// Installs a fake `sniper` build through staging + commit.
fn install(store: &Store, lock: &StoreLock, build: &str, make_current: MakeCurrent) -> Installed {
    let staged = store.staging_dir(lock, "test").unwrap().join("build");
    fs::create_dir(&staged).unwrap();
    fake_container(&staged, "sniper", build);
    let meta = BuildMeta::new(
        "sniper",
        &id(build),
        Source::Download {
            url: format!("https://example/{build}/x.tar.xz"),
            sha256: "00".repeat(32),
        },
    );
    store.commit(lock, &staged, meta, make_current).unwrap()
}

fn builds(store: &Store, name: &str) -> Vec<String> {
    store
        .installed_of(name)
        .unwrap()
        .iter()
        .map(|b| format!("{}{}", b.build, if b.is_current { "*" } else { "" }))
        .collect()
}

#[test]
fn empty_store() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path().join("missing"));
    assert!(store.installed().unwrap().is_empty());
    let err = store.resolve(&rref("sniper")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "runtime `sniper` is not installed; install it with `sruman -S sniper`"
    );
}

#[test]
fn commit_tracks_current_and_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();

    install(&store, &lock, "3.0.2", MakeCurrent::IfNewer);
    install(&store, &lock, "3.0.10", MakeCurrent::IfNewer);
    install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    assert_eq!(builds(&store, "sniper"), ["3.0.1", "3.0.2", "3.0.10*"]);

    assert_eq!(store.resolve(&rref("sniper")).unwrap().build, id("3.0.10"));
    assert_eq!(
        store.resolve(&rref("sniper=3.0.1")).unwrap().build,
        id("3.0.1")
    );
    let err = store.resolve(&rref("sniper=3.0.99")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "runtime `sniper` build 3.0.99 is not installed; install it with `sruman -S sniper=3.0.99`"
    );

    install(&store, &lock, "3.0.0", MakeCurrent::Always);
    assert_eq!(store.current("sniper"), Some(id("3.0.0")));
    store.set_current(&lock, "sniper", &id("3.0.2")).unwrap();
    assert_eq!(
        builds(&store, "sniper"),
        ["3.0.0", "3.0.1", "3.0.2*", "3.0.10"]
    );

    let meta = store.resolve(&rref("sniper=3.0.1")).unwrap().meta.unwrap();
    assert_eq!(meta.runtime, "sniper");
    assert_eq!(meta.sruman, crate::VERSION);
    assert!(matches!(meta.source, Source::Download { .. }));
}

#[test]
fn committing_the_same_build_twice_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    let staged = store.staging_dir(&lock, "again").unwrap();
    let meta = BuildMeta::new("sniper", &id("3.0.1"), Source::Import { from: "/x".into() });
    assert!(matches!(
        store.commit(&lock, &staged, meta, MakeCurrent::IfNewer),
        Err(StoreError::AlreadyInstalled { .. })
    ));
}

#[test]
fn resolve_falls_back_to_newest_without_current() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    install(&store, &lock, "3.0.2", MakeCurrent::IfNewer);
    fs::remove_file(tmp.path().join("sniper/current")).unwrap();
    assert_eq!(store.current("sniper"), None);
    assert_eq!(store.resolve(&rref("sniper")).unwrap().build, id("3.0.2"));
}

#[test]
fn missing_metadata_is_tolerated() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    fs::remove_file(tmp.path().join("sniper/3.0.1.toml")).unwrap();
    let build = store.resolve(&rref("sniper")).unwrap();
    assert_eq!(build.meta, None);
    assert!(!build.is_linked());
}

#[test]
fn store_lock_is_exclusive_and_wipes_staging() {
    let _spawn = SPAWN.read().unwrap_or_else(PoisonError::into_inner);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    let staged = store.staging_dir(&lock, "partial").unwrap();
    fs::write(staged.join("half.tar.xz"), "x").unwrap();

    assert!(matches!(store.lock(), Err(StoreError::Busy(_))));
    drop(lock);
    let _lock = store.lock().unwrap();
    assert!(!tmp.path().join(STAGING_DIR).exists());
}

#[test]
fn remove_repoints_current_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    let newest = install(&store, &lock, "3.0.2", MakeCurrent::IfNewer);

    store.remove(&lock, &newest, false).unwrap();
    assert_eq!(builds(&store, "sniper"), ["3.0.1*"]);
    assert!(!tmp.path().join("sniper/3.0.2.toml").exists());

    let last = store.resolve(&rref("sniper")).unwrap();
    store.remove(&lock, &last, false).unwrap();
    assert!(!tmp.path().join("sniper").exists());
    assert!(store.installed().unwrap().is_empty());
}

#[test]
fn remove_of_a_linked_build_only_unregisters_it() {
    let tmp = tempfile::tempdir().unwrap();
    let external = tmp.path().join("steam/SteamLinuxRuntime_sniper");
    fs::create_dir_all(&external).unwrap();
    fake_container(&external, "sniper", "3.0.5");
    let store = Store::open(tmp.path().join("store"));
    let lock = store.lock().unwrap();
    let meta = BuildMeta::new(
        "sniper",
        &id("3.0.5"),
        Source::Link {
            target: external.clone(),
        },
    );
    let linked = store
        .link(&lock, &external, meta, MakeCurrent::IfNewer)
        .unwrap();
    assert!(linked.is_linked());
    assert!(linked.is_current);

    store.remove(&lock, &linked, false).unwrap();
    assert!(store.installed().unwrap().is_empty());
    assert!(
        external.join("run").is_file(),
        "the linked target must survive"
    );
    // Nothing was written next to or inside the target.
    assert_eq!(fs::read_dir(external.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn hold_blocks_remove_unless_forced() {
    let _spawn = SPAWN.read().unwrap_or_else(PoisonError::into_inner);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    let build = install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    assert!(!store.in_use(&build));

    let hold = store.hold(&build).unwrap();
    assert!(store.in_use(&build));
    let err = store.remove(&lock, &build, false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "sniper 3.0.1 is in use by a running game; stop it first, or force the removal"
    );
    drop(hold);
    assert!(!store.in_use(&build));

    let _hold = store.hold(&build).unwrap();
    store.remove(&lock, &build, true).unwrap();
    assert!(store.installed().unwrap().is_empty());
}

#[test]
fn clean_keeps_current_newest_pinned_linked_and_in_use() {
    let _spawn = SPAWN.read().unwrap_or_else(PoisonError::into_inner);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path().join("store"));
    let lock = store.lock().unwrap();
    for build in ["3.0.1", "3.0.2", "3.0.3", "3.0.4", "3.0.5", "3.0.6"] {
        install(&store, &lock, build, MakeCurrent::IfNewer);
    }
    store.set_current(&lock, "sniper", &id("3.0.3")).unwrap();
    let busy = store.resolve(&rref("sniper=3.0.2")).unwrap();
    let _hold = store.hold(&busy).unwrap();
    let external = tmp.path().join("ext");
    fs::create_dir(&external).unwrap();
    let meta = BuildMeta::new(
        "sniper",
        &id("3.0.0"),
        Source::Link {
            target: external.clone(),
        },
    );
    store
        .link(&lock, &external, meta, MakeCurrent::IfNewer)
        .unwrap();

    let report = store
        .clean(
            &lock,
            &CleanPolicy {
                keep: 2,
                protect: vec![rref("sniper=3.0.4"), rref("sniper")],
            },
        )
        .unwrap();

    let removed: Vec<String> = report.removed.iter().map(|b| b.build.to_string()).collect();
    assert_eq!(removed, ["3.0.1"]);
    let kept: Vec<(String, KeptBecause)> = report
        .kept
        .iter()
        .map(|(b, why)| (b.build.to_string(), *why))
        .collect();
    assert_eq!(
        kept,
        [
            ("3.0.2".into(), KeptBecause::InUse),
            ("3.0.0".into(), KeptBecause::Linked)
        ]
    );
    assert_eq!(
        builds(&store, "sniper"),
        ["3.0.0", "3.0.2", "3.0.3*", "3.0.4", "3.0.5", "3.0.6"]
    );
}

/// pressure-vessel holds a read OFD lock on `var/tmp-*/.ref` while a game
/// runs. Simulated here with a child process (OFD locks need fcntl
/// constants std doesn't expose); skipped without python3.
#[test]
fn pressure_vessel_lock_marks_a_build_in_use() {
    let _spawn = SPAWN.write().unwrap_or_else(PoisonError::into_inner);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    let build = install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);
    let ref_dir = build.dir.join("var/tmp-ABC123");
    fs::create_dir_all(&ref_dir).unwrap();
    let ref_file = ref_dir.join(".ref");
    fs::write(&ref_file, "").unwrap();
    assert!(!store.in_use(&build));

    let script = "import fcntl,os,struct,sys,time\n\
                  fd=os.open(sys.argv[1],os.O_RDONLY)\n\
                  fcntl.fcntl(fd,fcntl.F_OFD_SETLK,struct.pack('hhqqi',fcntl.F_RDLCK,0,0,0,0))\n\
                  print('locked',flush=True)\n\
                  time.sleep(30)\n";
    let Ok(mut child) = Command::new("python3")
        .args(["-I", "-c", script])
        .arg(&ref_file)
        .stdout(std::process::Stdio::piped())
        .spawn()
    else {
        eprintln!("python3 not available; skipping");
        return;
    };
    let mut line = String::new();
    io::BufRead::read_line(
        &mut io::BufReader::new(child.stdout.as_mut().unwrap()),
        &mut line,
    )
    .unwrap();
    assert_eq!(line.trim(), "locked");

    let in_use = store.in_use(&build);
    let removal = store.remove(&lock, &build, false);
    let report = store.clean(&lock, &CleanPolicy::default());
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(in_use);
    assert!(matches!(removal, Err(StoreError::InUse { .. })));
    assert!(report.unwrap().removed.is_empty());
    assert!(!store.in_use(&build));
}

#[test]
fn verify_uses_the_builds_file_list() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    let build = install(&store, &lock, "3.0.1", MakeCurrent::IfNewer);

    let report = store.verify(&build, Depth::Content).unwrap().unwrap();
    assert_eq!(report.problems, []);
    assert_eq!(report.checked, 2);

    fs::write(build.dir.join("run"), "#!/bin/sh\nexit 1\n").unwrap();
    let report = store.verify(&build, Depth::Presence).unwrap().unwrap();
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);

    fs::remove_file(build.dir.join("mtree.txt.gz")).unwrap();
    assert_eq!(store.verify(&build, Depth::Content).unwrap(), None);
}

#[test]
fn remove_tree_handles_read_only_directories() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("ro");
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/f"), "x").unwrap();
    fs::set_permissions(dir.join("sub"), fs::Permissions::from_mode(0o555)).unwrap();
    remove_tree(&dir).unwrap();
    assert!(!dir.exists());
}
