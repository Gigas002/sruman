use std::{collections::HashMap, ffi::OsString, path::Path};

use super::*;

fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: HashMap<String, OsString> = vars
        .iter()
        .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
        .collect();
    move |key| vars.get(key).cloned()
}

#[test]
fn fallbacks_without_xdg_vars() {
    let xdg = XdgDirs::from_lookup(lookup(&[("HOME", "/home/u")])).unwrap();

    assert_eq!(xdg.config_home, Path::new("/home/u/.config"));
    assert_eq!(xdg.data_home, Path::new("/home/u/.local/share"));
    assert_eq!(xdg.state_home, Path::new("/home/u/.local/state"));
    assert_eq!(xdg.cache_home, Path::new("/home/u/.cache"));
    assert_eq!(xdg.runtime_dir, None);
}

#[test]
fn xdg_vars_override_fallbacks() {
    let xdg = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "/cfg"),
        ("XDG_DATA_HOME", "/data"),
        ("XDG_STATE_HOME", "/state"),
        ("XDG_CACHE_HOME", "/cache"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ]))
    .unwrap();

    assert_eq!(xdg.config_home, Path::new("/cfg"));
    assert_eq!(xdg.data_home, Path::new("/data"));
    assert_eq!(xdg.state_home, Path::new("/state"));
    assert_eq!(xdg.cache_home, Path::new("/cache"));
    assert_eq!(
        xdg.runtime_dir.as_deref(),
        Some(Path::new("/run/user/1000"))
    );
}

#[test]
fn empty_and_relative_xdg_vars_are_ignored() {
    let xdg = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", ""),
        ("XDG_CACHE_HOME", "relative/cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ]))
    .unwrap();

    assert_eq!(xdg.config_home, Path::new("/home/u/.config"));
    assert_eq!(xdg.cache_home, Path::new("/home/u/.cache"));
    assert_eq!(xdg.runtime_dir, None);
}

#[test]
fn missing_or_relative_home_is_an_error() {
    assert_eq!(XdgDirs::from_lookup(lookup(&[])), Err(PathsError::NoHome));
    assert_eq!(
        XdgDirs::from_lookup(lookup(&[("HOME", "home/u")])),
        Err(PathsError::NoHome)
    );
}

#[test]
fn app_dirs_layout() {
    let xdg = XdgDirs::from_lookup(lookup(&[
        ("HOME", "/home/u"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ]))
    .unwrap();
    let dirs = AppDirs::new(&xdg);

    assert_eq!(dirs.config, Path::new("/home/u/.config/sruman"));
    assert_eq!(dirs.profiles, Path::new("/home/u/.config/sruman/profiles"));
    assert_eq!(
        dirs.games_file,
        Path::new("/home/u/.config/sruman/games.toml")
    );
    assert_eq!(
        dirs.runtimes,
        Path::new("/home/u/.local/share/sruman/runtimes")
    );
    assert_eq!(
        dirs.prefixes,
        Path::new("/home/u/.local/share/sruman/prefixes")
    );
    assert_eq!(dirs.sync_cache, Path::new("/home/u/.cache/sruman/sync"));
    assert_eq!(dirs.pkg_cache, Path::new("/home/u/.cache/sruman/pkg"));
    assert_eq!(dirs.logs, Path::new("/home/u/.local/state/sruman/logs"));
    assert_eq!(dirs.locks, Path::new("/run/user/1000/sruman"));
}

#[test]
fn locks_fall_back_to_state_without_runtime_dir() {
    let xdg = XdgDirs::from_lookup(lookup(&[("HOME", "/home/u")])).unwrap();

    assert_eq!(
        AppDirs::new(&xdg).locks,
        Path::new("/home/u/.local/state/sruman/run")
    );
}

#[test]
fn expand_tilde_forms() {
    let home = Path::new("/home/u");

    assert_eq!(expand_tilde(Path::new("~"), home), Path::new("/home/u"));
    assert_eq!(
        expand_tilde(Path::new("~/Games/x"), home),
        Path::new("/home/u/Games/x")
    );
    assert_eq!(expand_tilde(Path::new("/abs"), home), Path::new("/abs"));
    assert_eq!(
        expand_tilde(Path::new("~other/x"), home),
        Path::new("~other/x")
    );
    assert_eq!(expand_tilde(Path::new("rel"), home), Path::new("rel"));
}
