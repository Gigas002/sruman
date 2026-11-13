use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;
use crate::model::Profile;

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    locks: PathBuf,
    home: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(tmp.path());
        let home = root.join("realhome");
        fs::create_dir(&home).unwrap();
        Self {
            locks: root.join("run/sruman"),
            home,
            root,
            _tmp: tmp,
        }
    }

    fn ctx(&self) -> PrefixContext<'_> {
        PrefixContext {
            lock_dir: &self.locks,
            home: &self.home,
        }
    }
}

fn usage<'a>(runtime: &'a str, profile: &'a str) -> PrefixUse<'a> {
    PrefixUse {
        runtime,
        build: "3.0.1",
        profile,
        home: HomeMode::Container,
    }
}

#[test]
fn ensure_creates_skeleton_and_records_use() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let prefix = env.root.join("prefixes/indie");
    let prepared = ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap();
    assert!(prepared.created);
    assert_eq!(prepared.home, prefix.join("home"));
    for dir in SKELETON {
        assert!(prefix.join("home").join(dir).is_dir(), "{dir}");
    }
    assert_eq!(prepared.state.sruman, crate::VERSION);
    assert_eq!(prepared.state.runtimes["sniper"].profile, "a");
    assert_eq!(read_state(&prefix), prepared.state);
    drop(prepared);

    // A second profile with another runtime adds to the state.
    let again = ensure(
        &prefix,
        &PrefixUse {
            home: HomeMode::Env,
            ..usage("scout-ldlp", "b")
        },
        &env.ctx(),
    )
    .unwrap();
    assert!(!again.created);
    assert_eq!(
        again.state.runtimes.keys().collect::<Vec<_>>(),
        ["scout-ldlp", "sniper"]
    );
    assert_eq!(again.state.runtimes["scout-ldlp"].home, HomeMode::Env);
    let text = fs::read_to_string(prefix.join(STATE_FILE)).unwrap();
    assert!(text.contains("home = \"env\""), "{text}");
    let leftovers: Vec<_> = fs::read_dir(&prefix)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn ensure_keeps_existing_files_and_tolerates_a_broken_state() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let prefix = env.root.join("p");
    fs::create_dir_all(prefix.join("home/.local/share/game")).unwrap();
    fs::write(prefix.join("home/.local/share/game/save"), "progress").unwrap();
    fs::write(prefix.join(STATE_FILE), "not = [valid").unwrap();
    let prepared = ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap();
    assert!(!prepared.created);
    assert_eq!(
        fs::read_to_string(prefix.join("home/.local/share/game/save")).unwrap(),
        "progress"
    );
    assert_eq!(read_state(&prefix).runtimes.len(), 1);
}

#[test]
fn shared_launches_coexist_and_block_reset_and_remove() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let prefix = env.root.join("p");
    let a = ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap();
    let b = ensure(&prefix, &usage("soldier", "b"), &env.ctx()).unwrap();
    assert!(in_use(&env.locks, &prefix));

    assert!(reset(&prefix, &env.ctx()).unwrap_err().is_busy());
    assert!(remove(&prefix, &env.ctx()).unwrap_err().is_busy());
    drop(a);
    assert!(in_use(&env.locks, &prefix));
    drop(b);
    assert!(!in_use(&env.locks, &prefix));

    // An exclusive holder keeps launches out.
    let exclusive = lock(&env.locks, &prefix, true).unwrap();
    assert!(matches!(
        ensure(&prefix, &usage("sniper", "a"), &env.ctx()),
        Err(PrefixError::Busy(_))
    ));
    drop(exclusive);
}

#[test]
fn locks_are_keyed_by_the_canonical_path() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let prefix = env.root.join("real");
    let link = env.root.join("link");
    let _held = ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap();
    symlink(&prefix, &link).unwrap();
    assert_eq!(lock_path(&env.locks, &link), lock_path(&env.locks, &prefix));
    assert_eq!(
        lock_path(&env.locks, &env.root.join("real/../real")),
        lock_path(&env.locks, &prefix)
    );
    assert!(in_use(&env.locks, &link));
    assert_ne!(
        lock_path(&env.locks, &prefix),
        lock_path(&env.locks, &env.root.join("other"))
    );
    let name = lock_path(&env.locks, &prefix)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("prefix-") && name.len() == "prefix-.lock".len() + 16,
        "{name}"
    );
    // Never locked: not in use, and no lock file is created by asking.
    assert!(!in_use(&env.locks, &env.root.join("never")));
}

#[test]
fn reset_empties_home_and_state_but_keeps_the_prefix() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let prefix = env.root.join("p");
    drop(ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap());
    let ro = prefix.join("home/.local/share/ro");
    fs::create_dir_all(ro.join("inner")).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o500)).unwrap();
    fs::write(prefix.join("notes.txt"), "mine").unwrap();

    reset(&prefix, &env.ctx()).unwrap();
    assert!(!prefix.join("home").exists());
    assert!(!prefix.join(STATE_FILE).exists());
    assert!(prefix.join("notes.txt").exists());
    // Resetting twice is fine.
    reset(&prefix, &env.ctx()).unwrap();
}

#[test]
fn remove_only_deletes_sruman_prefixes() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let plain = env.root.join("plain");
    fs::create_dir(&plain).unwrap();
    assert!(matches!(
        remove(&plain, &env.ctx()),
        Err(PrefixError::NotAPrefix(_))
    ));
    assert!(plain.exists());

    let prefix = env.root.join("p");
    drop(ensure(&prefix, &usage("sniper", "a"), &env.ctx()).unwrap());
    // A symlink inside the home must not be followed.
    symlink(&plain, prefix.join("home/escape")).unwrap();
    remove(&prefix, &env.ctx()).unwrap();
    assert!(!prefix.exists());
    assert!(plain.exists());
    remove(&prefix, &env.ctx()).unwrap();
}

#[test]
fn prefix_may_not_be_or_contain_the_home() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    for bad in [env.home.clone(), env.root.clone(), PathBuf::from("/")] {
        for result in [
            ensure(&bad, &usage("sniper", "a"), &env.ctx()).map(drop),
            reset(&bad, &env.ctx()),
            remove(&bad, &env.ctx()),
        ] {
            assert!(
                matches!(result, Err(PrefixError::ContainsHome(_))),
                "{}",
                bad.display()
            );
        }
    }
    // Through a symlink too.
    let alias = env.root.join("alias");
    symlink(&env.home, &alias).unwrap();
    assert!(matches!(
        reset(&alias, &env.ctx()),
        Err(PrefixError::ContainsHome(_))
    ));
    assert!(matches!(
        ensure(Path::new("rel"), &usage("sniper", "a"), &env.ctx()),
        Err(PrefixError::Relative(_))
    ));
    assert!(env.home.exists());
}

#[test]
fn sharing_lists_profiles_on_the_same_prefix() {
    let _spawn = crate::test_lock::checks();
    let env = Env::new();
    let mut profiles = LoadedProfiles::default();
    for (name, prefix) in [
        ("a", Some("~/p/indie")),
        ("b", Some("~/p/../p/indie")),
        ("c", Some("~/p/other")),
        ("d", None),
    ] {
        let mut profile = Profile::new(name, "sniper".parse().unwrap());
        profile.prefix = prefix.map(PathBuf::from);
        profiles.profiles.insert(name.into(), profile);
    }
    fs::create_dir_all(env.home.join("p/indie")).unwrap();
    assert_eq!(
        sharing(Path::new("~/p/indie"), &profiles, &env.home),
        ["a", "b"]
    );
    assert_eq!(
        sharing(&env.home.join("p/other"), &profiles, &env.home),
        ["c"]
    );
    assert!(sharing(Path::new("/nowhere"), &profiles, &env.home).is_empty());
}
