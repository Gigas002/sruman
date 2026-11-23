use std::fs;

use super::*;
use crate::store::tests::fake_container;

struct World {
    /// Builds' holds are checked, so none may overlap a test that spawns
    /// processes (see `test_lock`).
    _spawn: std::sync::RwLockReadGuard<'static, ()>,
    store: Store,
    catalog: Catalog,
    steam: Steam,
    tmp: tempfile::TempDir,
}

impl World {
    /// A Steam with its client runtime and steamrt4 in its root library.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Steam");
        fs::create_dir_all(root.join("steamapps/common")).unwrap();
        let w = Self {
            _spawn: crate::test_lock::checks(),
            store: Store::open(tmp.path().join("runtimes")),
            catalog: Catalog::builtin(),
            steam: Steam::at(&root, tmp.path().join("steam.pid")),
            tmp,
        };
        w.steam_copy(&root.join(CLIENT_DIR), "steamrt3c", "3c.0.1");
        w.steam_copy(
            &root.join("steamapps/common/SteamLinuxRuntime_4"),
            "steamrt4",
            "4.0.1",
        );
        w
    }

    /// Steam's copy of a runtime, with an old platform left from an update
    /// and pressure-vessel's `var/`.
    fn steam_copy(&self, dir: &Path, name: &str, build: &str) {
        fs::create_dir_all(dir).unwrap();
        fake_container(dir, name, build);
        fs::create_dir_all(dir.join(format!("{name}_platform_0.0.1/files"))).unwrap();
        fs::create_dir_all(dir.join("var/tmp-1")).unwrap();
    }

    fn slot(&self, runtime: &str) -> Slot {
        self.steam
            .slots()
            .into_iter()
            .find(|slot| slot.runtime == runtime)
            .unwrap()
    }

    fn link(&self, runtime: &str) -> Linked {
        self.steam
            .link(&self.store, &self.catalog, &self.slot(runtime))
            .unwrap()
    }
}

fn id(text: &str) -> BuildId {
    text.parse().unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn libraries_come_from_libraryfolders_vdf() {
    let w = World::new();
    let other = w.tmp.path().join("games");
    fs::create_dir_all(other.join("steamapps/common")).unwrap();
    w.steam_copy(
        &other.join("steamapps/common/SteamLinuxRuntime_soldier"),
        "soldier",
        "2.0.1",
    );
    let root = w.steam.root().display().to_string();
    fs::write(
        w.steam.root().join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{root}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"2\"\n\t{{\n\t\t\"path\"\t\t\"/nowhere\"\n\t}}\n}}\n",
            other.display()
        ),
    )
    .unwrap();
    assert_eq!(w.steam.libraries(), [w.steam.root().to_path_buf(), other]);
    let slots: Vec<_> = w
        .steam
        .slots()
        .into_iter()
        .map(|s| (s.runtime, s.client))
        .collect();
    assert_eq!(
        slots,
        [
            ("steamrt3c".to_owned(), true),
            ("steamrt4".to_owned(), false),
            ("soldier".to_owned(), false)
        ]
    );
}

#[test]
fn library_paths_unescape_backslashes() {
    assert_eq!(
        library_paths("\t\t\"path\"\t\t\"/a\\\\b\"\n\"label\"\t\"\"\n"),
        [PathBuf::from("/a\\b")]
    );
}

#[test]
fn link_takes_over_steams_copy() {
    let w = World::new();
    let slot = w.slot("steamrt4");
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Copy(Some(id("4.0.1")))
    );
    let linked = w.link("steamrt4");
    assert!(linked.changed && linked.imported && linked.damaged.is_none());
    assert_eq!(linked.build.build, id("4.0.1"));
    assert_eq!(fs::read_link(&slot.path).unwrap(), linked.build.dir);
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Linked(linked.build.clone())
    );
    // Neither pressure-vessel's scratch space nor the old platform came along.
    assert_eq!(
        names(&linked.build.dir),
        [
            "VERSIONS.txt",
            "mtree.txt.gz",
            "run",
            "steamrt4_platform_4.0.1"
        ]
    );
    assert!(
        names(slot.path.parent().unwrap())
            .iter()
            .all(|n| !n.starts_with('.'))
    );
    // Linking again changes nothing.
    let again = w.link("steamrt4");
    assert!(!again.changed && !again.imported);
    // A link through `current` is pinned to the build.
    fs::remove_file(&slot.path).unwrap();
    symlink(
        linked.build.dir.parent().unwrap().join("current"),
        &slot.path,
    )
    .unwrap();
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Linked(linked.build.clone())
    );
    assert!(w.link("steamrt4").changed);
    assert_eq!(fs::read_link(&slot.path).unwrap(), linked.build.dir);
    assert_eq!(
        w.steam.linked(&w.store),
        [(slot.clone(), linked.build.clone())]
    );
}

#[test]
fn link_uses_an_installed_build() {
    let w = World::new();
    let dir = w.tmp.path().join("elsewhere");
    w.steam_copy(&dir, "steamrt3c", "3c.0.1");
    let lock = w.store.lock().unwrap();
    let installed =
        import::import(&w.store, &lock, &w.catalog, &dir, None, ImportMode::Copy).unwrap();
    drop(lock);
    let linked = w.link("steamrt3c");
    assert!(linked.changed && !linked.imported);
    assert_eq!(linked.build, installed);
    assert_eq!(w.store.installed_of("steamrt3c").unwrap().len(), 1);
}

#[test]
fn an_update_through_the_link_is_imported_and_relinked() {
    let w = World::new();
    let old = w.link("steamrt4").build;
    // Steam updates the runtime in place, through the link.
    fake_container(&old.dir, "steamrt4", "4.0.2");
    let slot = w.slot("steamrt4");
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Changed {
            build: old.clone(),
            now: Some(id("4.0.2"))
        }
    );
    // Still kept by clean and remove until it's relinked.
    assert_eq!(w.steam.linked(&w.store).len(), 1);
    let linked = w.link("steamrt4");
    assert!(linked.changed && linked.imported);
    assert_eq!(linked.build.build, id("4.0.2"));
    assert_eq!(linked.damaged, Some(old));
    assert_eq!(fs::read_link(&slot.path).unwrap(), linked.build.dir);
    assert_eq!(
        names(&linked.build.dir),
        [
            "VERSIONS.txt",
            "mtree.txt.gz",
            "run",
            "steamrt4_platform_4.0.2"
        ]
    );
}

#[test]
fn a_damaged_copy_is_left_alone() {
    let w = World::new();
    let slot = w.slot("steamrt4");
    fs::write(slot.path.join("run"), "truncated").unwrap();
    let err = w.steam.link(&w.store, &w.catalog, &slot).unwrap_err();
    assert!(matches!(err, SteamError::Damaged { count: 1, .. }), "{err}");
    assert!(
        !fs::symlink_metadata(&slot.path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn unlink_puts_a_copy_back() {
    let w = World::new();
    let slot = w.slot("steamrt3c");
    let build = w.link("steamrt3c").build;
    assert!(w.steam.unlink(&w.store, &slot).unwrap());
    let meta = fs::symlink_metadata(&slot.path).unwrap();
    assert!(meta.is_dir() && !meta.file_type().is_symlink());
    assert_eq!(names(&slot.path), names(&build.dir));
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Copy(Some(id("3c.0.1")))
    );
    // The build stays installed; a second unlink has nothing to do.
    assert!(build.dir.is_dir());
    assert!(!w.steam.unlink(&w.store, &slot).unwrap());
    assert!(w.steam.linked(&w.store).is_empty());
}

#[test]
fn a_foreign_link_is_refused() {
    let w = World::new();
    let slot = w.slot("steamrt4");
    fs::remove_dir_all(&slot.path).unwrap();
    symlink("/nowhere", &slot.path).unwrap();
    assert_eq!(
        slot.state(&w.store).unwrap(),
        State::Foreign("/nowhere".into())
    );
    for err in [
        w.steam.link(&w.store, &w.catalog, &slot).unwrap_err(),
        w.steam.unlink(&w.store, &slot).unwrap_err(),
    ] {
        assert!(matches!(err, SteamError::Foreign { .. }), "{err}");
    }
}

#[test]
fn a_stale_pid_file_isnt_a_running_steam() {
    let w = World::new();
    assert!(!w.steam.running());
    // This test process isn't called `steam`.
    fs::write(
        w.tmp.path().join("steam.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    assert!(!w.steam.running());
}

#[test]
fn locate_prefers_the_configured_dir() {
    let w = World::new();
    let home = w.tmp.path().join("home");
    let data = w.tmp.path().join("data");
    assert!(Steam::locate(None, &home, &data).is_none());
    fs::create_dir_all(home.join(".steam")).unwrap();
    symlink(w.steam.root(), home.join(".steam/root")).unwrap();
    let found = Steam::locate(None, &home, &data).unwrap();
    assert_eq!(found.root(), fs::canonicalize(w.steam.root()).unwrap());
    assert!(Steam::locate(Some(&data), &home, &data).is_none());
}
