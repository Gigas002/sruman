use std::path::Path;

use std::collections::BTreeMap;

use super::*;
use crate::version::RuntimeRef;

const HOME: &str = "/home/u";

fn ctx(catalog: &Catalog) -> ValidationContext<'_> {
    ValidationContext {
        home: Path::new(HOME),
        catalog,
    }
}

fn fixtures(dir: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/profiles")
        .join(dir)
}

fn messages(issues: &[Issue]) -> Vec<String> {
    issues.iter().map(ToString::to_string).collect()
}

#[test]
fn names_follow_lootrs_rules() {
    for ok in ["sniper", "a.b_c-1", "X"] {
        assert!(validate_name(ok).is_ok(), "{ok}");
    }
    for bad in ["", ".", "..", "-p", "a b", "a/b", "ü"] {
        assert!(validate_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn good_fixtures_load_and_validate() {
    let catalog = Catalog::builtin();
    let loaded = load_dir(&fixtures("good"));
    assert_eq!(messages(&loaded.issues), Vec::<String>::new());
    assert_eq!(
        loaded.profiles.keys().collect::<Vec<_>>(),
        ["full", "minimal"]
    );

    let full = &loaded.profiles["full"];
    assert_eq!(full.slr.runtime.to_string(), "sniper=3.0.20261021.266025");
    assert_eq!(full.wrappers, ["mangohud --dlsym", "gamemoderun"]);
    assert_eq!(
        full.shader_cache,
        Some(ShaderCache::Dir("~/.cache/shaders".into()))
    );
    assert!(full.debug);
    for profile in loaded.profiles.values() {
        assert_eq!(
            messages(&profile.validate(&ctx(&catalog))),
            Vec::<String>::new()
        );
    }
}

#[test]
fn bad_files_are_reported_and_the_rest_load() {
    let loaded = load_dir(&fixtures("bad"));
    let text = messages(&loaded.issues);
    assert!(
        text.iter()
            .any(|m| m.contains("broken.toml: invalid profile")),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|m| m.contains("no-slr.toml: invalid profile") && m.contains("slr")),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|m| m
                .contains("mismatch.toml: `name = \"other\"` must match the file name `mismatch`")),
        "{text:#?}"
    );
    assert!(
        text.contains(&"warning: profile `invalid`: unknown key `typo_key` is ignored".to_owned())
    );
    assert!(
        text.contains(&"warning: profile `invalid`: unknown key `slr.extra` is ignored".to_owned())
    );
    assert_eq!(
        loaded.profiles.keys().collect::<Vec<_>>(),
        ["invalid", "ldlp"]
    );
}

#[test]
fn validation_collects_every_problem() {
    let catalog = Catalog::builtin();
    let loaded = load_dir(&fixtures("bad"));
    let text = messages(&loaded.profiles["invalid"].validate(&ctx(&catalog)));
    assert_eq!(
        text,
        [
            "error: profile `invalid`: `prefix` must be an absolute path (or start with `~`), got `relative/prefix`",
            "error: profile `invalid`: `shader_cache` must be an absolute path (or start with `~`), got `cache`",
            "error: profile `invalid`: `wrappers` must not contain empty entries",
            "error: profile `invalid`: `env` key `` is not a valid variable name",
            "error: profile `invalid`: `slr.runtime`: unknown runtime `snipper`; known: scout, scout-ldlp, sniper, soldier, steamrt4",
        ]
    );
}

#[test]
fn scout_ldlp_warns_about_what_it_cannot_do() {
    let catalog = Catalog::builtin();
    let loaded = load_dir(&fixtures("bad"));
    let issues = loaded.profiles["ldlp"].validate(&ctx(&catalog));
    assert!(issues.iter().all(|i| !i.is_error()), "{issues:#?}");
    let text = messages(&issues).join("\n");
    assert!(
        text.contains("`slr.filesystems_ro` is ignored for `scout-ldlp`"),
        "{text}"
    );
    assert!(text.contains("only redirects `HOME` and `XDG_*`"), "{text}");
    assert!(text.contains("`runner_args` is ignored"), "{text}");
}

#[test]
fn slr_section_validates_on_its_own() {
    let catalog = Catalog::builtin();
    let section = SlrSection::new("nope".parse().unwrap());
    let issues = section.validate(
        Subject::Profile("lootrs-native".into()),
        false,
        &ctx(&catalog),
    );
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0]
            .to_string()
            .starts_with("error: profile `lootrs-native`: `slr.runtime`: unknown runtime `nope`")
    );
}

#[test]
fn implicit_profiles_fill_in_for_catalog_runtimes() {
    let catalog = Catalog::builtin();
    let loaded = load_dir(&fixtures("good"));
    let sniper = loaded.get("sniper", &catalog).unwrap();
    assert_eq!(sniper, Profile::new("sniper", "sniper".parse().unwrap()));
    assert_eq!(sniper.prefix, None);
    assert!(loaded.get("full", &catalog).unwrap().debug);
    assert!(loaded.get("nothing", &catalog).is_none());

    let mut shadowing = loaded.clone();
    shadowing.profiles.insert(
        "soldier".into(),
        Profile::new("soldier", "sniper".parse().unwrap()),
    );
    assert_eq!(
        shadowing.get("soldier", &catalog).unwrap().slr.runtime.name,
        "sniper"
    );
    assert_eq!(
        shadowing.implicit_names(&catalog).collect::<Vec<_>>(),
        ["scout", "scout-ldlp", "sniper", "steamrt4"]
    );
}

#[test]
fn save_round_trips_and_is_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let mut profile = Profile::new("mine", RuntimeRef::new("sniper", None).unwrap());
    profile.prefix = Some("~/p".into());
    profile.env.insert("A".into(), "1".into());
    profile.slr.filesystems_rw.push("~/Games".into());

    let path = save_profile(tmp.path(), &profile).unwrap();
    assert_eq!(path, tmp.path().join("mine.toml"));
    let loaded = load_dir(tmp.path());
    assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
    assert_eq!(loaded.profiles["mine"], profile);
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "no temp file left"
    );

    let bad = Profile::new("../x", RuntimeRef::new("sniper", None).unwrap());
    assert!(matches!(
        save_profile(tmp.path(), &bad),
        Err(ModelError::InvalidName(_))
    ));
}

#[test]
fn missing_directory_is_empty() {
    let loaded = load_dir(Path::new("/nonexistent/sruman/profiles"));
    assert!(loaded.profiles.is_empty() && loaded.issues.is_empty());
}

#[test]
fn shader_cache_forms() {
    assert_eq!(ShaderCache::parse(" OFF "), ShaderCache::Off);
    assert_eq!(ShaderCache::parse("~/c").as_text(), "~/c");
    let vars: Vec<&str> = ShaderCache::env(Path::new("/c"))
        .iter()
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(
        vars,
        [
            "MESA_SHADER_CACHE_DIR",
            "__GL_SHADER_DISK_CACHE_PATH",
            "AMD_VK_PIPELINE_CACHE_PATH"
        ]
    );
}

fn games_fixture() -> LoadedGames {
    load_games(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/games/games.toml"))
}

fn game(name: &str) -> Game {
    Game {
        name: name.into(),
        exe: format!("/g/{name}").into(),
        profile: None,
        args: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
    }
}

#[test]
fn games_load_with_duplicates_and_broken_entries_reported() {
    let loaded = games_fixture();
    let names: Vec<&str> = loaded.games.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["good", "bad paths", "default"]);
    let good = loaded.get("good").unwrap();
    assert_eq!(good.exe, Path::new("~/g/good.x86_64"));
    assert_eq!(good.args, ["-a"]);
    assert_eq!(good.env["A"], "1");

    let text = messages(&loaded.issues);
    assert!(
        text.contains(
            &"error: game `good`: duplicate name; only the first entry is used".to_owned()
        ),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|m| m.contains("invalid `[[game]]` no-exe") && m.contains("exe")),
        "{text:#?}"
    );
    assert!(
        text.contains(&"warning: game `bad paths`: unknown key `colour` is ignored".to_owned())
    );
    assert_eq!(text.len(), 3, "{text:#?}");
}

#[test]
fn games_validate_against_profiles_and_default() {
    let catalog = Catalog::builtin();
    let loaded = games_fixture();
    let profiles = LoadedProfiles::default();

    let text = messages(&loaded.validate(&profiles, Some("soldier"), &ctx(&catalog)));
    assert!(
        text.contains(&"warning: game `good`: `exe` not found: /home/u/g/good.x86_64".to_owned())
    );
    for expected in [
        "error: game `bad paths`: name `bad paths` contains ` `",
        "error: game `bad paths`: profile `missing` does not exist",
        "error: game `bad paths`: `exe` must be an absolute path (or start with `~`), got `relative/exe`",
        "error: game `bad paths`: `cwd` must be an absolute path (or start with `~`), got `rel`",
    ] {
        assert!(
            text.iter().any(|m| m.starts_with(expected)),
            "{expected}\n{text:#?}"
        );
    }
    assert!(
        !text.iter().any(|m| m.contains("game `default`: profile")),
        "{text:#?}"
    );

    let without_default = messages(&loaded.validate(&profiles, None, &ctx(&catalog)));
    assert!(without_default.contains(
        &"error: game `default`: no `profile` set and no usable `default_profile` in config.toml".to_owned()
    ));
    let bad_default = messages(&loaded.validate(&profiles, Some("nope"), &ctx(&catalog)));
    assert!(
        bad_default
            .iter()
            .any(|m| m.starts_with("error: game `default`: no `profile` set"))
    );
}

#[test]
fn game_profile_overrides_default() {
    let mut g = game("x");
    assert_eq!(g.profile_name(Some("sniper")), Some("sniper"));
    assert_eq!(g.profile_name(None), None);
    g.profile = Some("scout".into());
    assert_eq!(g.profile_name(Some("sniper")), Some("scout"));
}

#[test]
fn games_file_missing_or_broken() {
    assert!(
        load_games(Path::new("/nonexistent/games.toml"))
            .games
            .is_empty()
    );
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("games.toml");
    std::fs::write(&path, "[[game]\n").unwrap();
    let loaded = load_games(&path);
    assert!(loaded.games.is_empty());
    assert!(messages(&loaded.issues)[0].contains("invalid games file"));

    std::fs::write(&path, "game = 1\nother = 2\n").unwrap();
    let text = messages(&load_games(&path).issues);
    assert!(
        text.iter().any(|m| m.contains("unknown key `other`")),
        "{text:#?}"
    );
    assert!(
        text.iter().any(|m| m.contains("invalid games file")),
        "{text:#?}"
    );
}

#[test]
fn targets_resolve_like_a_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path();
    std::fs::write(cwd.join("local"), "").unwrap();
    std::fs::write(cwd.join("supertux"), "").unwrap();
    let home = Path::new(HOME);
    let games = [game("supertux"), game("hollow-knight")];

    // A game name wins, even over a file of the same name.
    assert_eq!(
        resolve_target("supertux", &games, cwd, home),
        Ok(RunTarget::Game(&games[0]))
    );
    // `/` or `~` makes it a path.
    assert_eq!(
        resolve_target("./local", &games, cwd, home),
        Ok(RunTarget::Path(cwd.join("./local")))
    );
    assert_eq!(
        resolve_target("bin/x", &games, cwd, home),
        Ok(RunTarget::Path(cwd.join("bin/x")))
    );
    assert_eq!(
        resolve_target("/opt/x", &games, cwd, home),
        Ok(RunTarget::Path("/opt/x".into()))
    );
    assert_eq!(
        resolve_target("~/x", &games, cwd, home),
        Ok(RunTarget::Path("/home/u/x".into()))
    );
    assert_eq!(
        resolve_target("~", &games, cwd, home),
        Ok(RunTarget::Path(home.into()))
    );
    // A path that names a game is still a path.
    assert_eq!(
        resolve_target("./supertux", &games, cwd, home),
        Ok(RunTarget::Path(cwd.join("./supertux")))
    );

    // An unknown name never runs the local file; it only hints.
    let err = resolve_target("local", &games, cwd, home).unwrap_err();
    assert_eq!(err.to_string(), "no game 'local' (did you mean ./local?)");
    assert_eq!(
        err,
        TargetError::UnknownGame {
            name: "local".into(),
            local: Some(cwd.join("local"))
        }
    );
    let err = resolve_target("nothing", &games, cwd, home).unwrap_err();
    assert_eq!(err.to_string(), "no game 'nothing'");
    // A directory is not a hint.
    std::fs::create_dir(cwd.join("dir")).unwrap();
    assert_eq!(
        resolve_target("dir", &games, cwd, home)
            .unwrap_err()
            .to_string(),
        "no game 'dir'"
    );
    assert_eq!(
        resolve_target("", &games, cwd, home),
        Err(TargetError::Empty)
    );
}

#[test]
fn prefix_may_not_swallow_the_home() {
    let catalog = Catalog::builtin();
    for prefix in ["~", "/home", "/"] {
        let mut profile = Profile::new("p", "sniper".parse().unwrap());
        profile.prefix = Some(prefix.into());
        let text = messages(&profile.validate(&ctx(&catalog)));
        assert!(
            text.iter().any(|m| m.contains("is or contains your home")),
            "{prefix}: {text:#?}"
        );
    }
    let mut profile = Profile::new("p", "sniper".parse().unwrap());
    profile.prefix = Some("~/prefixes/p".into());
    assert!(profile.validate(&ctx(&catalog)).is_empty());
}
