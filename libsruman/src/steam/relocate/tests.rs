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
        assert!(matches!(err, SteamError::Foreign { .. }), "{err}");
    }
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
