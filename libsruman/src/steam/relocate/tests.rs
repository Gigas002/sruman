use std::fs;

use super::*;

struct World {
    steam: Steam,
    targets: Targets,
    tmp: tempfile::TempDir,
}

impl World {
    /// A Steam with a library holding a game and a shader cache, an
    /// appcache and logs; no depotcache yet.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Steam");
        for dir in [
            "steamapps/common/Game",
            "steamapps/shadercache/1",
            "appcache/librarycache",
            "logs",
            "config/htmlcache",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("steamapps/common/Game/game"), "game").unwrap();
        fs::write(root.join("appcache/appinfo.vdf"), "info").unwrap();
        fs::write(root.join("config/config.vdf"), "config").unwrap();
        let targets = Targets {
            library: Some(tmp.path().join("games/steam")),
            cache: tmp.path().join("cache/steam"),
            state: tmp.path().join("state/steam"),
        };
        Self {
            steam: Steam::at(&root, tmp.path().join("steam.pid")),
            targets,
            tmp,
        }
    }

    fn places(&self) -> Vec<Place> {
        self.steam.places(&self.targets)
    }

    fn place(&self, name: &str) -> Place {
        self.places().into_iter().find(|p| p.name == name).unwrap()
    }
}

#[test]
fn places_list_the_library_first() {
    let w = World::new();
    let names: Vec<_> = w.places().iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "library",
            "appcache",
            "depotcache",
            "package",
            "htmlcache",
            "avatarcache",
            "shadercache",
            "logs"
        ]
    );
    let none = Targets {
        library: None,
        ..w.targets.clone()
    };
    assert!(w.steam.places(&none).iter().all(|p| p.name != "library"));
    assert_eq!(w.place("appcache").placed(), Placed::Here);
    assert_eq!(w.place("depotcache").placed(), Placed::Missing);
}

#[test]
fn relocate_moves_everything_and_links_it() {
    let w = World::new();
    for place in w.places() {
        assert!(w.steam.relocate(&place).unwrap(), "{}", place.name);
        assert_eq!(place.placed(), Placed::Moved, "{}", place.name);
        assert_eq!(fs::read_link(&place.path).unwrap(), place.target);
    }
    let root = w.steam.root();
    // Through the links, Steam finds what it had.
    assert_eq!(
        fs::read_to_string(root.join("steamapps/common/Game/game")).unwrap(),
        "game"
    );
    assert_eq!(
        fs::read_to_string(w.tmp.path().join("games/steam/steamapps/common/Game/game")).unwrap(),
        "game"
    );
    assert_eq!(
        fs::read_to_string(w.targets.cache.join("appcache/appinfo.vdf")).unwrap(),
        "info"
    );
    assert!(w.targets.cache.join("shadercache/1").is_dir());
    assert!(w.targets.cache.join("depotcache").is_dir());
    assert!(w.targets.state.join("logs").is_dir());
    // config.vdf stays; only its cache moved.
    assert!(
        !fs::symlink_metadata(root.join("config"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Again: nothing to do.
    for place in w.places() {
        assert!(!w.steam.relocate(&place).unwrap());
    }
}

#[test]
fn unrelocate_moves_it_back() {
    let w = World::new();
    for place in w.places() {
        w.steam.relocate(&place).unwrap();
    }
    for place in w.places().iter().rev() {
        assert!(w.steam.unrelocate(place).unwrap(), "{}", place.name);
        assert_eq!(place.placed(), Placed::Here, "{}", place.name);
    }
    let root = w.steam.root();
    assert_eq!(
        fs::read_to_string(root.join("steamapps/common/Game/game")).unwrap(),
        "game"
    );
    assert!(root.join("steamapps/shadercache/1").is_dir());
    assert!(!w.tmp.path().join("games/steam/steamapps").exists());
    assert!(!w.steam.unrelocate(&w.place("logs")).unwrap());
}

#[test]
fn a_full_target_is_refused() {
    let w = World::new();
    let place = w.place("appcache");
    fs::create_dir_all(place.target.join("old")).unwrap();
    let err = w.steam.relocate(&place).unwrap_err();
    assert!(matches!(err, SteamError::TargetExists(_)), "{err}");
    assert_eq!(place.placed(), Placed::Here);
    // An empty one is fine.
    fs::remove_dir(place.target.join("old")).unwrap();
    assert!(w.steam.relocate(&place).unwrap());
}

#[test]
fn a_move_cut_short_is_finished() {
    let w = World::new();
    let place = w.place("appcache");
    fs::create_dir_all(place.target.parent().unwrap()).unwrap();
    fs::rename(&place.path, &place.target).unwrap();
    assert_eq!(place.placed(), Placed::Missing);
    assert!(w.steam.relocate(&place).unwrap());
    assert!(place.path.join("appinfo.vdf").is_file());
}

#[test]
fn a_link_elsewhere_is_left_alone() {
    let w = World::new();
    let place = w.place("logs");
    fs::remove_dir(&place.path).unwrap();
    symlink("/nowhere", &place.path).unwrap();
    assert_eq!(place.placed(), Placed::Elsewhere("/nowhere".into()));
    for err in [
        w.steam.relocate(&place).unwrap_err(),
        w.steam.unrelocate(&place).unwrap_err(),
    ] {
        assert!(matches!(err, SteamError::Relocated { .. }), "{err}");
    }
}

#[test]
fn moved_with_other_settings_says_so() {
    let mut w = World::new();
    let place = w.place("appcache");
    w.steam.relocate(&place).unwrap();
    w.targets.cache = w.tmp.path().join("other-cache");
    let err = w.steam.relocate(&w.place("appcache")).unwrap_err();
    assert!(err.to_string().contains("--undo"), "{err}");
    assert!(matches!(err, SteamError::Relocated { .. }), "{err}");
}

#[test]
fn a_target_inside_the_folder_is_refused() {
    let mut w = World::new();
    w.targets.library = Some(w.steam.root().join("steamapps/common/x/.."));
    let err = w.steam.relocate(&w.place("library")).unwrap_err();
    assert!(matches!(err, SteamError::Nested { .. }), "{err}");
    // Or Steam's own root as the library: the target is the folder itself.
    w.targets.library = Some(w.steam.root().to_path_buf());
    let err = w.steam.relocate(&w.place("library")).unwrap_err();
    assert!(matches!(err, SteamError::Nested { .. }), "{err}");
    assert_eq!(w.place("library").placed(), Placed::Here);
}

#[test]
fn moved_folders_stay_private() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    fs::set_permissions(w.steam.root(), fs::Permissions::from_mode(0o700)).unwrap();
    for place in w.places() {
        w.steam.relocate(&place).unwrap();
        assert_eq!(mode(&place.target), 0o700, "{}", place.name);
    }
    assert_eq!(mode(&w.targets.cache), 0o700);
    assert_eq!(mode(w.targets.library.as_ref().unwrap()), 0o700);
    // A Steam folder others may read lets the moved folders be read too.
    let w = World::new();
    fs::set_permissions(w.steam.root(), fs::Permissions::from_mode(0o755)).unwrap();
    let place = w.place("logs");
    fs::set_permissions(&place.path, fs::Permissions::from_mode(0o755)).unwrap();
    w.steam.relocate(&place).unwrap();
    assert_eq!(mode(&place.target), 0o755);
}

/// Runs a copy of bash as `exe`, in `dir`, waiting. (coreutils may be one
/// binary that goes by its own name, so `sleep` can't be renamed.)
fn sleeper(exe: &Path, dir: &Path) -> std::process::Child {
    let bash = ["/usr/bin/bash", "/bin/bash"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap();
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::copy(bash, exe).unwrap();
    let script = exe.with_extension("sh");
    // A builtin, so there's no child process to outlive bash.
    fs::write(&script, "read -t 30\n").unwrap();
    let _spawn = crate::test_lock::spawns();
    #[allow(clippy::disallowed_methods)]
    std::process::Command::new(exe)
        .arg(&script)
        .stdin(std::process::Stdio::piped())
        .current_dir(dir)
        .spawn()
        .unwrap()
}

#[test]
fn a_folder_in_use_is_refused() {
    let w = World::new();
    let place = w.place("library");
    let dir = place.path.join("common/Game");
    let mut child = sleeper(&w.tmp.path().join("bin/game"), &dir);
    let err = w.steam.relocate(&place).unwrap_err();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        matches!(err, SteamError::Busy { ref name, .. } if name == "game"),
        "{err}"
    );
    assert_eq!(place.placed(), Placed::Here);
    // And back: a process in the moved folder blocks the undo.
    w.steam.relocate(&place).unwrap();
    let mut child = sleeper(&w.tmp.path().join("bin/game"), &place.target);
    let err = w.steam.unrelocate(&place).unwrap_err();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(matches!(err, SteamError::Busy { .. }), "{err}");
}

#[test]
fn a_steam_from_this_root_counts_as_running() {
    let w = World::new();
    let root = w.steam.root().to_path_buf();
    let mut child = sleeper(&root.join("ubuntu12_32/steam"), w.tmp.path());
    let running = w.steam.running();
    let other = Steam::at(w.tmp.path().join("other"), w.tmp.path().join("none.pid"));
    let other_running = other.running();
    let err = w.steam.relocate(&w.place("logs")).unwrap_err();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(running && !other_running);
    assert!(matches!(err, SteamError::Running), "{err}");
}

#[test]
fn the_pid_file_of_the_roots_home_counts() {
    let w = World::new();
    let home = w.tmp.path().join("home");
    let root = home.join(".local/share/Steam");
    fs::create_dir_all(&root).unwrap();
    let mut child = sleeper(&w.tmp.path().join("bin/steam"), w.tmp.path());
    fs::create_dir_all(home.join(".steam")).unwrap();
    fs::write(home.join(".steam/steam.pid"), child.id().to_string()).unwrap();
    let steam = Steam::at(&root, w.tmp.path().join("none.pid"));
    let running = steam.running();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(running);
}

#[test]
fn a_move_across_filesystems_copies() {
    let w = World::new();
    let from = w.tmp.path().join("from");
    fs::create_dir_all(from.join("a/b")).unwrap();
    fs::write(from.join("a/b/f"), "x").unwrap();
    symlink("b/f", from.join("a/l")).unwrap();
    let to = w.tmp.path().join("to");
    copy_move(&from, &to).unwrap();
    assert!(!from.exists());
    assert_eq!(fs::read_to_string(to.join("a/l")).unwrap(), "x");
}
