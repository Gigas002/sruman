//! The runtimes sruman knows how to install: a built-in table that matches
//! Valve's image server, which `config.toml` `[[runtime]]` entries can extend
//! or override (a mirror, a new suite), plus dependency ordering.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::version::{self, Channel};

#[cfg(test)]
mod tests;

/// Valve's image server.
pub const DEFAULT_BASE_URL: &str = "https://repo.steampowered.com";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("unknown runtime `{name}`{}", required_by.as_ref().map(|by| format!(" (required by `{by}`)")).unwrap_or_default())]
    Unknown {
        name: String,
        required_by: Option<String>,
    },
    #[error("runtime dependency cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    #[error("[[runtime]] `{name}`: {message}")]
    InvalidOverride { name: String, message: String },
}

/// CPU architecture of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    Arm64,
}

impl Arch {
    /// The architecture this build of sruman runs on, if it's one Valve
    /// publishes runtimes for.
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm64 => "arm64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a runtime starts a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Entry {
    /// pressure-vessel container: `<build>/run -- <cmd>`.
    Container,
    /// scout's libraries inside a soldier container:
    /// `<soldier>/run -- <build>/scout-on-soldier-entry-point-v2 -- <cmd>`.
    ScoutOnSoldier,
    /// No container, the LD_LIBRARY_PATH runtime: `<build>/run.sh <cmd>`.
    Ldlp,
}

impl Entry {
    /// The file inside a build directory that identifies this kind.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Container => "run",
            Self::ScoutOnSoldier => "scout-on-soldier-entry-point-v2",
            Self::Ldlp => "run.sh",
        }
    }
}

/// One installable runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDef {
    /// The name users type: `sniper`.
    pub name: String,
    pub description: String,
    /// Server root, without a trailing `/`.
    pub base_url: String,
    /// Directory under the root: `steamrt3`. Builds live in
    /// `<base_url>/<server_dir>/images/<build_id>/`.
    pub server_dir: String,
    /// Archive file name per architecture.
    pub archives: BTreeMap<Arch, String>,
    pub entry: Entry,
    /// Runtimes that must be installed first.
    pub depends: Vec<String>,
    /// Per-channel pointer files, overriding [`Channel::pointer_files`].
    pub pointers: BTreeMap<String, Vec<String>>,
}

impl RuntimeDef {
    fn builtin(
        name: &str,
        description: &str,
        server_dir: &str,
        archives: &[(Arch, &str)],
        entry: Entry,
        depends: &[&str],
    ) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            server_dir: server_dir.to_owned(),
            archives: archives
                .iter()
                .map(|(arch, file)| (*arch, (*file).to_owned()))
                .collect(),
            entry,
            depends: depends.iter().map(|dep| (*dep).to_owned()).collect(),
            pointers: BTreeMap::new(),
        }
    }

    /// `<base_url>/<server_dir>/images`.
    pub fn images_url(&self) -> String {
        format!("{}/{}/images", self.base_url, self.server_dir)
    }

    pub fn archive(&self, arch: Arch) -> Option<&str> {
        self.archives.get(&arch).map(String::as_str)
    }

    /// Pointer files to try for `channel`, in order.
    pub fn pointer_files(&self, channel: Channel) -> Vec<&str> {
        match self.pointers.get(channel.as_str()) {
            Some(files) => files.iter().map(String::as_str).collect(),
            None => channel.pointer_files().to_vec(),
        }
    }
}

/// A `[[runtime]]` entry from `config.toml`. For a built-in name, every set
/// field replaces the built-in one; a new name needs `server_dir`,
/// `archives` and `entry`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOverride {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archives: Option<BTreeMap<Arch, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    /// `{ stable = ["latest-public-stable"], ... }`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointers: Option<BTreeMap<String, Vec<String>>>,
}

/// Every runtime sruman can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    runtimes: BTreeMap<String, RuntimeDef>,
}

impl Catalog {
    /// The runtimes Valve publishes, as checked on 2026-11-13.
    pub fn builtin() -> Self {
        use Arch::{Arm64, X86_64};
        let defs = [
            RuntimeDef::builtin(
                "scout",
                "Steam Linux Runtime 1.0 (scout), inside a soldier container",
                "steamrt1",
                &[(X86_64, "SteamLinuxRuntime.tar.xz")],
                Entry::ScoutOnSoldier,
                &["soldier"],
            ),
            RuntimeDef::builtin(
                "scout-ldlp",
                "Steam Linux Runtime 1.0 (scout), LD_LIBRARY_PATH only: no container",
                "steamrt1",
                &[(X86_64, "steam-runtime.tar.xz")],
                Entry::Ldlp,
                &[],
            ),
            RuntimeDef::builtin(
                "soldier",
                "Steam Linux Runtime 2.0 (soldier), Debian 10 based",
                "steamrt2",
                &[(X86_64, "SteamLinuxRuntime_soldier.tar.xz")],
                Entry::Container,
                &[],
            ),
            RuntimeDef::builtin(
                "sniper",
                "Steam Linux Runtime 3.0 (sniper), Debian 11 based",
                "steamrt3",
                &[(X86_64, "SteamLinuxRuntime_sniper.tar.xz")],
                Entry::Container,
                &[],
            ),
            RuntimeDef::builtin(
                "steamrt4",
                "Steam Linux Runtime 4.0, Debian 13 based, 64-bit only",
                "steamrt4",
                &[
                    (X86_64, "SteamLinuxRuntime_4.tar.xz"),
                    (Arm64, "SteamLinuxRuntime_4-arm64.tar.xz"),
                ],
                Entry::Container,
                &[],
            ),
        ];
        Self {
            runtimes: defs
                .into_iter()
                .map(|def| (def.name.clone(), def))
                .collect(),
        }
    }

    /// Applies `[[runtime]]` entries on top of this catalog. Collects every
    /// invalid entry instead of stopping at the first.
    pub fn with_overrides(
        mut self,
        overrides: &[RuntimeOverride],
    ) -> Result<Self, Vec<CatalogError>> {
        let mut errors = Vec::new();
        for over in overrides {
            let invalid = |message: &str| CatalogError::InvalidOverride {
                name: over.name.clone(),
                message: message.to_owned(),
            };
            if let Err(err) = version::validate_runtime_name(&over.name) {
                errors.push(invalid(&err.to_string()));
                continue;
            }
            let def = match self.runtimes.get(&over.name) {
                Some(existing) => existing.clone(),
                None => {
                    let (Some(server_dir), Some(archives), Some(entry)) =
                        (&over.server_dir, &over.archives, over.entry)
                    else {
                        errors.push(invalid(
                            "a new runtime needs `server_dir`, `archives` and `entry`",
                        ));
                        continue;
                    };
                    RuntimeDef {
                        name: over.name.clone(),
                        description: String::new(),
                        base_url: DEFAULT_BASE_URL.to_owned(),
                        server_dir: server_dir.clone(),
                        archives: archives.clone(),
                        entry,
                        depends: Vec::new(),
                        pointers: BTreeMap::new(),
                    }
                }
            };
            let def = RuntimeDef {
                description: over.description.clone().unwrap_or(def.description),
                base_url: over
                    .base_url
                    .as_deref()
                    .map(|url| url.trim_end_matches('/').to_owned())
                    .unwrap_or(def.base_url),
                server_dir: over.server_dir.clone().unwrap_or(def.server_dir),
                archives: over.archives.clone().unwrap_or(def.archives),
                entry: over.entry.unwrap_or(def.entry),
                depends: over.depends.clone().unwrap_or(def.depends),
                pointers: over.pointers.clone().unwrap_or(def.pointers),
                name: def.name,
            };
            if !base_url_allowed(&def.base_url) {
                errors.push(invalid(
                    "`base_url` must start with `https://` (`http://` only for localhost)",
                ));
                continue;
            }
            if def.server_dir.is_empty()
                || def.server_dir.contains(['/', '\\'])
                || def.server_dir.starts_with('.')
            {
                errors.push(invalid("`server_dir` must be one plain directory name"));
                continue;
            }
            if def
                .archives
                .values()
                .any(|file| file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.'))
            {
                errors.push(invalid("archive names must be plain file names"));
                continue;
            }
            if let Some(key) = def
                .pointers
                .keys()
                .find(|key| key.parse::<Channel>().is_err())
            {
                errors.push(invalid(&format!("`pointers.{key}` is not a channel")));
                continue;
            }
            self.runtimes.insert(def.name.clone(), def);
        }
        if errors.is_empty() {
            Ok(self)
        } else {
            Err(errors)
        }
    }

    pub fn get(&self, name: &str) -> Option<&RuntimeDef> {
        self.runtimes.get(name)
    }

    /// All runtimes, by name.
    pub fn iter(&self) -> impl Iterator<Item = &RuntimeDef> {
        self.runtimes.values()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.runtimes.keys().map(String::as_str)
    }

    /// `names` plus everything they depend on, dependencies first (the
    /// order to install in). Each runtime appears once.
    pub fn install_order<'a, I, S>(&self, names: I) -> Result<Vec<&RuntimeDef>, CatalogError>
    where
        I: IntoIterator<Item = &'a S>,
        S: AsRef<str> + ?Sized + 'a,
    {
        let mut order = Vec::new();
        let mut done = std::collections::BTreeSet::new();
        for name in names {
            let mut path = Vec::new();
            self.visit(name.as_ref(), None, &mut path, &mut done, &mut order)?;
        }
        Ok(order)
    }

    fn visit<'s>(
        &'s self,
        name: &str,
        required_by: Option<&str>,
        path: &mut Vec<String>,
        done: &mut std::collections::BTreeSet<String>,
        order: &mut Vec<&'s RuntimeDef>,
    ) -> Result<(), CatalogError> {
        if done.contains(name) {
            return Ok(());
        }
        if let Some(start) = path.iter().position(|seen| seen == name) {
            let mut cycle = path[start..].to_vec();
            cycle.push(name.to_owned());
            return Err(CatalogError::Cycle(cycle));
        }
        let def = self.get(name).ok_or_else(|| CatalogError::Unknown {
            name: name.to_owned(),
            required_by: required_by.map(str::to_owned),
        })?;
        path.push(name.to_owned());
        for dep in &def.depends {
            self.visit(dep, Some(name), path, done, order)?;
        }
        path.pop();
        done.insert(name.to_owned());
        order.push(def);
        Ok(())
    }

    /// Runtimes that depend on `name` directly (`-R` refuses to remove a
    /// dependency that's still needed).
    pub fn dependents(&self, name: &str) -> Vec<&RuntimeDef> {
        self.iter()
            .filter(|def| def.depends.iter().any(|dep| dep == name))
            .collect()
    }
}

/// `https://` anywhere; plain `http://` only to this machine (a local
/// mirror or a test server), since `SHA256SUMS` comes over the same
/// connection as the archives it checks.
fn base_url_allowed(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1") && !authority.contains('@')
}
