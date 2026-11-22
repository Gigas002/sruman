//! Profiles: `profiles/<name>.toml`, in the libgameprofiles format shared
//! with lootrs and gaki.
//!
//! A sruman profile has `runner = "slr"` and an `[slr]` section: the runtime,
//! the mode (container or overlay) and what the game sees. libgameprofiles checks the file itself;
//! [`validate_profile`] adds what a sruman launch needs: the runner, a mode
//! sruman supports, and a runtime the catalog knows.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use libgameprofiles::{LibOverride, Profile, Runner, ShaderCache, SlrMode, SlrSection, store};

use super::{Issue, Issues, ModelError, Subject, ValidationContext};
use crate::{catalog::Catalog, overlay, version::RuntimeRef};

/// The `[slr]` section of a profile sruman can run, and its parsed runtime,
/// or why it can't run it.
pub fn slr(profile: &Profile) -> Result<(&SlrSection, RuntimeRef), String> {
    match profile.runner {
        Runner::Slr => {}
        // The format before `slr` became a runner.
        Runner::Native if profile.slr.is_some() => {
            return Err(
                "runner `native` doesn't use `[slr]`: set `runner = \"slr\"` for sruman".to_owned(),
            );
        }
        runner => {
            return Err(format!(
                "runner `{runner}` isn't sruman's: sruman runs `slr` profiles"
            ));
        }
    }
    let Some(slr) = &profile.slr else {
        return Err(MISSING_SLR.to_owned());
    };
    let runtime = slr
        .runtime
        .parse::<RuntimeRef>()
        .map_err(|e| format!("`slr.runtime`: {e}"))?;
    Ok((slr, runtime))
}

/// libgameprofiles' own error for an `slr` profile without `[slr]`.
const MISSING_SLR: &str = "`[slr]` is required for runner `slr`: it names the runtime";

/// scout's LD_LIBRARY_PATH runtime was its own catalog entry before D13.
const SCOUT_LDLP: &str = "scout-ldlp";

/// A `native` profile running in `runtime`, with nothing else set.
pub fn new_profile(name: impl Into<String>, runtime: &RuntimeRef) -> Profile {
    Profile {
        slr: Some(SlrSection::new(runtime.to_string())),
        ..Profile::new(name, Runner::Slr)
    }
}

/// What's wrong with `profile` for sruman: libgameprofiles' check of the
/// file, then what a launch needs (a runner and `[slr]` sruman runs, a known
/// runtime) and what its runtime can't do.
pub fn validate_profile(profile: &Profile, ctx: &ValidationContext) -> Vec<Issue> {
    let mut issues = Issues::new(Subject::Profile(profile.name.clone()));
    issues.extend(profile.validate(ctx.home));
    match slr(profile) {
        Ok((slr, runtime)) => check_slr(slr, &runtime, ctx.catalog, &mut issues),
        // libgameprofiles has already reported it.
        Err(message) if message == MISSING_SLR => {}
        Err(message) => issues.error(message),
    }
    issues.finish()
}

/// The runtime must be a catalog one, and `[slr.lib_overrides]` can't ask
/// for the runtime's glibc or GPU libraries (D14). libgameprofiles warns
/// about settings a mode ignores.
fn check_slr(slr: &SlrSection, runtime: &RuntimeRef, catalog: &Catalog, issues: &mut Issues) {
    for (lib, setting) in &slr.lib_overrides {
        if *setting == LibOverride::Runtime && overlay::host_only(lib) {
            let what = if overlay::is_glibc(lib) {
                "glibc"
            } else {
                "the GPU stack"
            };
            issues.error(format!(
                "`slr.lib_overrides.\"{lib}\"` can't be \"runtime\": {what} always comes from \
                 the host in overlay mode"
            ));
        }
    }
    let name = &runtime.name;
    if name == SCOUT_LDLP && catalog.get(name).is_none() {
        issues.error(format!(
            "`slr.runtime`: `{SCOUT_LDLP}` is now `scout` with `mode = \"overlay\"`"
        ));
        return;
    }
    if catalog.get(name).is_none() {
        let known: Vec<&str> = catalog.names().collect();
        issues.error(format!(
            "`slr.runtime`: unknown runtime `{name}`; known: {}",
            known.join(", ")
        ));
    }
}

/// Profiles loaded from a directory, plus the issues found while loading.
#[derive(Debug, Clone, Default)]
pub struct LoadedProfiles {
    /// By name. Only files that parsed.
    pub profiles: BTreeMap<String, Profile>,
    /// Unreadable files, name/file-stem mismatches and unknown keys.
    pub issues: Vec<Issue>,
}

impl LoadedProfiles {
    /// The profile file `name`, or the implicit profile for a catalog
    /// runtime of that name (no prefix, default settings), so
    /// `sruman -p sniper ./game` works without any file.
    pub fn get(&self, name: &str, catalog: &Catalog) -> Option<Profile> {
        self.profiles
            .get(name)
            .cloned()
            .or_else(|| implicit_profile(name, catalog))
    }

    /// Names of the implicit profiles not shadowed by a file.
    pub fn implicit_names<'a>(&'a self, catalog: &'a Catalog) -> impl Iterator<Item = &'a str> {
        catalog
            .names()
            .filter(|name| !self.profiles.contains_key(*name))
    }
}

/// The implicit profile for catalog runtime `name`.
pub fn implicit_profile(name: &str, catalog: &Catalog) -> Option<Profile> {
    let def = catalog.get(name)?;
    let runtime = RuntimeRef::new(&def.name, None).ok()?;
    Some(new_profile(&def.name, &runtime))
}

/// Loads every `*.toml` in `dir` (a missing directory is empty). A broken
/// file is reported and the rest still load.
pub fn load_dir(dir: &Path) -> LoadedProfiles {
    let loaded = store::load_dir(dir);
    let issues = loaded.issues().collect();
    LoadedProfiles {
        profiles: loaded
            .profiles
            .into_iter()
            .map(|profile| (profile.name.clone(), profile))
            .collect(),
        issues,
    }
}

/// Writes `profiles/<name>.toml` atomically.
pub fn save_profile(dir: &Path, profile: &Profile) -> Result<PathBuf, ModelError> {
    store::save(dir, profile).map_err(ModelError::from)
}

pub(super) fn file_issue(path: &Path, message: String) -> Issue {
    Issue::error(Subject::File(path.to_path_buf()), message)
}
