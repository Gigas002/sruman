//! Profiles: `profiles/<name>.toml`, in the libgameprofiles format shared
//! with lootrs and gaki.
//!
//! A sruman profile is a `native` profile with an `[slr]` section: the
//! runtime and what its container sees. libgameprofiles checks the file
//! itself; [`validate_profile`] adds what a sruman launch needs: the runner,
//! the section, and a runtime the catalog knows.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use libgameprofiles::{Profile, Runner, ShaderCache, SlrSection, store};

use super::{Issue, Issues, ModelError, Subject, ValidationContext};
use crate::{
    catalog::{Catalog, Entry},
    version::RuntimeRef,
};

/// The `[slr]` section of a profile sruman can run, and its parsed runtime,
/// or why it can't run it.
pub fn slr(profile: &Profile) -> Result<(&SlrSection, RuntimeRef), String> {
    if profile.runner != Runner::Native {
        return Err(format!(
            "runner `{}` isn't sruman's: sruman runs `native` games",
            profile.runner
        ));
    }
    let Some(slr) = &profile.slr else {
        return Err(
            "`[slr]` is required: it names the Steam Linux Runtime the games run in".to_owned(),
        );
    };
    let runtime = slr
        .runtime
        .parse::<RuntimeRef>()
        .map_err(|e| format!("`slr.runtime`: {e}"))?;
    Ok((slr, runtime))
}

/// A `native` profile running in `runtime`, with nothing else set.
pub fn new_profile(name: impl Into<String>, runtime: &RuntimeRef) -> Profile {
    Profile {
        slr: Some(SlrSection::new(runtime.to_string())),
        ..Profile::new(name, Runner::Native)
    }
}

/// What's wrong with `profile` for sruman: libgameprofiles' check of the
/// file, then what a launch needs (a runner and `[slr]` sruman runs, a known
/// runtime) and what its runtime can't do.
pub fn validate_profile(profile: &Profile, ctx: &ValidationContext) -> Vec<Issue> {
    let mut issues = Issues::new(Subject::Profile(profile.name.clone()));
    issues.extend(profile.validate(ctx.home));
    match slr(profile) {
        Ok((slr, runtime)) => check_slr(slr, &runtime, profile, ctx.catalog, &mut issues),
        Err(message) => issues.error(message),
    }
    issues.finish()
}

/// The runtime must be a catalog one, and an `ldlp` runtime (no container)
/// ignores some settings.
fn check_slr(
    slr: &SlrSection,
    runtime: &RuntimeRef,
    profile: &Profile,
    catalog: &Catalog,
    issues: &mut Issues,
) {
    let name = &runtime.name;
    let Some(def) = catalog.get(name) else {
        let known: Vec<&str> = catalog.names().collect();
        issues.error(format!(
            "`slr.runtime`: unknown runtime `{name}`; known: {}",
            known.join(", ")
        ));
        return;
    };
    if def.entry != Entry::Ldlp {
        return;
    }
    for (field, paths) in [
        ("slr.filesystems_rw", &slr.filesystems_rw),
        ("slr.filesystems_ro", &slr.filesystems_ro),
    ] {
        if !paths.is_empty() {
            issues.warn(format!(
                "`{field}` is ignored for `{name}`: it has no container, so everything is shared"
            ));
        }
    }
    if profile.prefix.is_some() {
        issues.warn(format!(
            "`{name}` has no container: the prefix home only redirects `HOME` and `XDG_*`, \
             so a game that looks up its home another way still reaches the real one"
        ));
    }
    if !profile.runner_args.is_empty() {
        issues.warn(format!(
            "`runner_args` is ignored for `{name}`: it has no container"
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
