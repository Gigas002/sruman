use std::fs;

use super::*;
use crate::store::{BuildMeta, MakeCurrent, Source, tests::fake_container};

struct World {
    store: Store,
    folders: Folders,
    _tmp: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Self {
            store: Store::open(tmp.path().join("runtimes")),
            folders: Folders::new(tmp.path()),
            _tmp: tmp,
        }
    }

    fn install(&self, build: &str) -> Installed {
        let lock = self.store.lock().unwrap();
        let staged = self.store.staging_dir(&lock, "t").unwrap().join("b");
        fs::create_dir(&staged).unwrap();
        fake_container(&staged, "sniper", build);
        fs::write(staged.join("_v2-entry-point"), "#!/bin/sh\n").unwrap();
        let meta = BuildMeta::new(
            "sniper",
            &build.parse().unwrap(),
            Source::Import { from: "/x".into() },
        );
        self.store
            .commit(&lock, &staged, meta, MakeCurrent::Always)
            .unwrap()
    }
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
fn variants_follow_umu() {
    assert_eq!(variant(1_391_110), Some("steamrt2"));
    assert_eq!(variant(1_628_350), Some("steamrt3"));
    assert_eq!(variant(4_183_110), Some("steamrt4"));
    assert_eq!(variant(1), None);
    let catalog = crate::catalog::Catalog::builtin();
    assert_eq!(catalog.by_app_id(1_628_350).unwrap().name, "sniper");
    assert!(catalog.get("scout").unwrap().app_id.is_none());
}

#[test]
fn a_view_is_symlinks_plus_umu_link_and_marker() {
    let w = World::new();
    let build = w.install("3.0.1");
    let provided = w.folders.provide(&w.store, &build, "steamrt3").unwrap();
    let dir = w.folders.root().join("umu/steamrt3");
    assert_eq!(
        provided,
        Provided {
            dir: dir.clone(),
            build: build.build.clone(),
            kept: false,
            changed: true,
        }
    );
    assert_eq!(
        names(&dir),
        [
            ".installed.ok",
            ".sruman-view",
            "VERSIONS.txt",
            "_v2-entry-point",
            "mtree.txt.gz",
            "run",
            "sniper_platform_3.0.1",
            "umu",
        ]
    );
    assert_eq!(
        fs::read_link(dir.join("run")).unwrap(),
        build.dir.join("run")
    );
    assert_eq!(
        fs::read_link(dir.join("umu")).unwrap(),
        Path::new("_v2-entry-point")
    );
    assert_eq!(fs::read_to_string(dir.join(MARKER)).unwrap(), "ok\n");
    assert_eq!(
        w.folders.shown("steamrt3"),
        Some(("sniper".into(), build.build.clone()))
    );
    // The build itself is untouched.
    assert!(!build.dir.join("umu").exists() && !build.dir.join(MARKER).exists());

    // Asking again changes nothing.
    let again = w.folders.provide(&w.store, &build, "steamrt3").unwrap();
    assert!(!again.changed && !again.kept);
}

#[test]
fn an_upgrade_repoints_the_view_unless_the_old_build_runs() {
    let w = World::new();
    let old = w.install("3.0.1");
    w.folders.provide(&w.store, &old, "steamrt3").unwrap();
    let new = w.install("3.0.2");

    // A game on the old build: the view stays.
    let hold = w.store.hold(&old).unwrap();
    let provided = w.folders.provide(&w.store, &new, "steamrt3").unwrap();
    assert!(provided.kept && !provided.changed);
    assert_eq!(provided.build, old.build);
    drop(hold);

    let provided = w.folders.provide(&w.store, &new, "steamrt3").unwrap();
    assert!(provided.changed && !provided.kept);
    let dir = &provided.dir;
    assert_eq!(fs::read_link(dir.join("run")).unwrap(), new.dir.join("run"));
    assert!(dir.join("sniper_platform_3.0.2").exists());
    assert!(!dir.join("sniper_platform_3.0.1").exists());
    // No staging left behind.
    assert_eq!(names(&w.folders.root().join("umu")), ["steamrt3"]);
}

#[test]
fn broken_or_foreign_views_are_replaced() {
    let w = World::new();
    let build = w.install("3.0.1");
    // umu's own download, made before sruman provided the runtime.
    let dir = w.folders.view_dir("steamrt3");
    fs::create_dir_all(dir.join("pressure-vessel/bin")).unwrap();
    fs::write(dir.join(MARKER), "ok\n").unwrap();
    let provided = w.folders.provide(&w.store, &build, "steamrt3").unwrap();
    assert!(provided.changed);
    assert!(!dir.join("pressure-vessel").exists());

    // The build went away under the view (removed with `--force`).
    let lock = w.store.lock().unwrap();
    w.store.remove(&lock, &build, true).unwrap();
    drop(lock);
    assert!(!intact(&dir));
    let build = w.install("3.0.2");
    let provided = w.folders.provide(&w.store, &build, "steamrt3").unwrap();
    assert!(provided.changed && intact(&provided.dir));
}

#[test]
fn the_store_lock_is_needed_to_change_a_view() {
    let w = World::new();
    let build = w.install("3.0.1");
    let _lock = w.store.lock().unwrap();
    let err = w.folders.provide(&w.store, &build, "steamrt3").unwrap_err();
    assert!(matches!(err, UmuError::Store(StoreError::Busy(_))), "{err}");
}
