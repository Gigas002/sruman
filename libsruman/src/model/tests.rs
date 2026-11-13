use std::path::Path;

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
