//! Every file under `examples/` loads and validates.

use std::path::{Path, PathBuf};

use libsruman::{
    catalog::Catalog,
    model::{self, ValidationContext},
};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

#[test]
fn example_profiles_load_and_validate_without_errors() {
    let catalog = Catalog::builtin();
    let ctx = ValidationContext {
        home: Path::new("/home/u"),
        catalog: &catalog,
    };
    let loaded = model::load_dir(&examples().join("profiles"));
    assert!(loaded.issues.is_empty(), "{:#?}", loaded.issues);
    assert_eq!(
        loaded.profiles.keys().collect::<Vec<_>>(),
        ["scout", "shared-prefix-a", "shared-prefix-b", "sniper"]
    );
    for profile in loaded.profiles.values() {
        let issues = profile.validate(&ctx);
        assert!(issues.is_empty(), "{}: {issues:#?}", profile.name);
    }
}

#[test]
fn example_games_load_and_validate() {
    let catalog = Catalog::builtin();
    let ctx = ValidationContext {
        home: Path::new("/home/u"),
        catalog: &catalog,
    };
    let profiles = model::load_dir(&examples().join("profiles"));
    let games = model::load_games(&examples().join("games.toml"));
    assert!(games.issues.is_empty(), "{:#?}", games.issues);
    assert_eq!(games.games.len(), 3);
    // The exes don't exist here, which is only a warning.
    let issues = games.validate(&profiles, Some("sniper"), &ctx);
    assert!(issues.iter().all(|i| !i.is_error()), "{issues:#?}");
}
