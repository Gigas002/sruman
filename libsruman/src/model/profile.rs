use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{Issue, Issues, ModelError, Subject, ValidationContext, unknown_keys, write_atomic};
use crate::{catalog::Entry, version::RuntimeRef};

/// Keys a profile file may have; anything else gets a warning.
const KNOWN_KEYS: &[&str] = &[
    "name",
    "prefix",
    "runner_args",
    "wrappers",
    "debug",
    "env.*",
    "shader_cache",
    "slr",
    "slr.runtime",
    "slr.filesystems_rw",
    "slr.filesystems_ro",
];

/// A profile from `profiles/<name>.toml`: how a game runs.
///
/// The fields besides `slr` mean what they mean in a lootrs profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Must equal the file stem.
    pub name: String,
    /// The game's private home lives in `<prefix>/home`. Several profiles
    /// may share a prefix. Unset: the game sees the real home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PathBuf>,
    /// Passed to the runtime entry point (pressure-vessel options).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runner_args: Vec<String>,
    /// Prepended to the whole command, outside the container, like Steam's
    /// `mangohud %command%`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrappers: Vec<String>,
    /// Verbose runtime logging.
    #[serde(default, skip_serializing_if = "is_false")]
    pub debug: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Where each game's compiled shaders go. Unset leaves the drivers'
    /// defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader_cache: Option<ShaderCache>,
    pub slr: SlrSection,
}

/// `[slr]`: which runtime, and what the container sees. lootrs embeds this
/// section in its own profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlrSection {
    /// `sniper`, or `sniper=<build>` to pin a build.
    pub runtime: RuntimeRef,
    /// Extra host paths shared read-write with the container.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems_rw: Vec<PathBuf>,
    /// Extra host paths shared read-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems_ro: Vec<PathBuf>,
}

impl SlrSection {
    pub fn new(runtime: RuntimeRef) -> Self {
        Self {
            runtime,
            filesystems_rw: Vec::new(),
            filesystems_ro: Vec::new(),
        }
    }

    /// Checks the section for `subject` (a sruman or lootrs profile, which
    /// says whether it has a prefix).
    pub fn validate(
        &self,
        subject: Subject,
        has_prefix: bool,
        ctx: &ValidationContext,
    ) -> Vec<Issue> {
        let mut issues = Issues::new(subject);
        self.check(has_prefix, ctx, &mut issues);
        issues.finish()
    }

    pub(crate) fn check(&self, has_prefix: bool, ctx: &ValidationContext, issues: &mut Issues) {
        let name = &self.runtime.name;
        let Some(def) = ctx.catalog.get(name) else {
            let known: Vec<&str> = ctx.catalog.names().collect();
            issues.error(format!(
                "`slr.runtime`: unknown runtime `{name}`; known: {}",
                known.join(", ")
            ));
            return;
        };
        for (field, paths) in [
            ("slr.filesystems_rw", &self.filesystems_rw),
            ("slr.filesystems_ro", &self.filesystems_ro),
        ] {
            for path in paths {
                issues.absolute(field, path, ctx.home);
            }
            if def.entry == Entry::Ldlp && !paths.is_empty() {
                issues.warn(format!(
                    "`{field}` is ignored for `{name}`: it has no container, so everything is shared"
                ));
            }
        }
        if def.entry == Entry::Ldlp && has_prefix {
            issues.warn(format!(
                "`{name}` has no container: the prefix home only redirects `HOME` and `XDG_*`, \
                 so a game that looks up its home another way still reaches the real one"
            ));
        }
    }
}

/// `shader_cache`: `"off"`, or a directory that holds one folder per game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderCache {
    Off,
    Dir(PathBuf),
}

impl ShaderCache {
    pub fn parse(text: &str) -> Self {
        if text.trim().eq_ignore_ascii_case("off") {
            Self::Off
        } else {
            Self::Dir(PathBuf::from(text))
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::Off => "off".to_owned(),
            Self::Dir(dir) => dir.display().to_string(),
        }
    }

    /// The variables that put one game's shader caches in `dir`: Mesa,
    /// NVIDIA and AMD's Vulkan driver (DXVK/VKD3D don't apply to native
    /// games). Same layout as lootrs.
    pub fn env(dir: &Path) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("MESA_SHADER_CACHE_DIR", dir.to_path_buf()),
            ("__GL_SHADER_DISK_CACHE_PATH", dir.join("nvidiav1")),
            ("AMD_VK_PIPELINE_CACHE_PATH", dir.join("AMDv1")),
        ]
    }
}

impl Serialize for ShaderCache {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_text())
    }
}

impl<'de> Deserialize<'de> for ShaderCache {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.trim().is_empty() {
            return Err(serde::de::Error::custom(
                "`shader_cache` is `off` or a directory",
            ));
        }
        Ok(Self::parse(&text))
    }
}

impl Profile {
    /// A profile with only `name` and the runtime set.
    pub fn new(name: impl Into<String>, runtime: RuntimeRef) -> Self {
        Self {
            name: name.into(),
            prefix: None,
            runner_args: Vec::new(),
            wrappers: Vec::new(),
            debug: false,
            env: BTreeMap::new(),
            shader_cache: None,
            slr: SlrSection::new(runtime),
        }
    }

    pub fn validate(&self, ctx: &ValidationContext) -> Vec<Issue> {
        let mut issues = Issues::new(Subject::Profile(self.name.clone()));
        issues.name(&self.name);
        if let Some(prefix) = &self.prefix {
            issues.absolute("prefix", prefix, ctx.home);
        }
        if let Some(ShaderCache::Dir(dir)) = &self.shader_cache {
            issues.absolute("shader_cache", dir, ctx.home);
        }
        issues.wrappers(&self.wrappers);
        issues.env(&self.env);
        self.slr.check(self.prefix.is_some(), ctx, &mut issues);
        if !self.runner_args.is_empty()
            && ctx
                .catalog
                .get(&self.slr.runtime.name)
                .is_some_and(|def| def.entry == Entry::Ldlp)
        {
            issues.warn(format!(
                "`runner_args` is ignored for `{}`: it has no container",
                self.slr.runtime.name
            ));
        }
        issues.finish()
    }

    /// Parses one profile file's text; unknown keys become warnings.
    pub fn parse(text: &str, subject: Subject) -> Result<(Self, Vec<Issue>), String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        let profile: Self = value
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let mut issues = Issues::new(subject);
        unknown_keys(&value, KNOWN_KEYS, &mut issues);
        Ok((profile, issues.finish()))
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| e.to_string())
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
    pub fn get(&self, name: &str, catalog: &crate::catalog::Catalog) -> Option<Profile> {
        self.profiles
            .get(name)
            .cloned()
            .or_else(|| implicit_profile(name, catalog))
    }

    /// Names of the implicit profiles not shadowed by a file.
    pub fn implicit_names<'a>(
        &'a self,
        catalog: &'a crate::catalog::Catalog,
    ) -> impl Iterator<Item = &'a str> {
        catalog
            .names()
            .filter(|name| !self.profiles.contains_key(*name))
    }
}

/// The implicit profile for catalog runtime `name`.
pub fn implicit_profile(name: &str, catalog: &crate::catalog::Catalog) -> Option<Profile> {
    let def = catalog.get(name)?;
    let runtime = RuntimeRef::new(&def.name, None).ok()?;
    Some(Profile::new(&def.name, runtime))
}

/// Loads every `*.toml` in `dir` (a missing directory is empty). A broken
/// file is reported and the rest still load.
pub fn load_dir(dir: &Path) -> LoadedProfiles {
    let mut loaded = LoadedProfiles::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return loaded,
        Err(e) => {
            loaded
                .issues
                .push(file_issue(dir, format!("can't read the directory: {e}")));
            return loaded;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml") && path.is_file())
        .collect();
    paths.sort();
    for path in paths {
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                loaded
                    .issues
                    .push(file_issue(&path, format!("can't read: {e}")));
                continue;
            }
        };
        match Profile::parse(&text, Subject::Profile(stem.clone())) {
            Ok((profile, mut issues)) => {
                if profile.name != stem {
                    loaded.issues.push(file_issue(
                        &path,
                        format!(
                            "`name = \"{}\"` must match the file name `{stem}`",
                            profile.name
                        ),
                    ));
                    continue;
                }
                loaded.issues.append(&mut issues);
                loaded.profiles.insert(stem, profile);
            }
            Err(message) => loaded
                .issues
                .push(file_issue(&path, format!("invalid profile: {message}"))),
        }
    }
    loaded
}

/// Writes `profiles/<name>.toml` atomically.
pub fn save_profile(dir: &Path, profile: &Profile) -> Result<PathBuf, ModelError> {
    super::validate_name(&profile.name).map_err(ModelError::InvalidName)?;
    let path = dir.join(format!("{}.toml", profile.name));
    let text = profile.to_toml().map_err(|message| ModelError::Serialize {
        path: path.clone(),
        message,
    })?;
    write_atomic(&path, &text)?;
    Ok(path)
}

fn file_issue(path: &Path, message: String) -> Issue {
    Issue {
        severity: super::Severity::Error,
        subject: Subject::File(path.to_path_buf()),
        message,
    }
}

fn is_false(value: &bool) -> bool {
    !value
}
