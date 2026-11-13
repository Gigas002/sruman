//! A runtime build's package list: Valve's `*.manifest.dpkg`, with the
//! Debian package and version of everything in the build.
//!
//! sruman uses it the way apt uses its index: `-Si`/`-Qi` list packages,
//! upgrades show which packages change, and `-Ss --pkg` searches them.

use std::{collections::BTreeMap, fmt};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("manifest line {line}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    /// `amd64`, `i386`, or `None` for `Architecture: all` packages.
    pub arch: Option<String>,
    pub version: String,
    /// Source package, when it differs from `name` (with its version in
    /// parentheses when that differs too).
    pub source: Option<String>,
    pub installed_size_kib: Option<u64>,
}

impl Package {
    /// `name` or `name:arch`.
    pub fn key(&self) -> String {
        match &self.arch {
            Some(arch) => format!("{}:{arch}", self.name),
            None => self.name.clone(),
        }
    }
}

/// The packages of one build, keyed by `name[:arch]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    packages: BTreeMap<String, Package>,
}

impl Manifest {
    /// Parses `manifest.dpkg`: tab-separated
    /// `Package[:Architecture] Version Source Installed-Size`, with `#`
    /// header lines.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut packages = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_no = index + 1;
            let err = |message: &str| ParseError {
                line: line_no,
                message: message.to_owned(),
            };
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [package, version, source, size] = fields[..] else {
                return Err(err(&format!(
                    "expected 4 tab-separated fields, found {}",
                    fields.len()
                )));
            };
            let (name, arch) = match package.split_once(':') {
                Some((name, arch)) => (name, Some(arch)),
                None => (package, None),
            };
            if name.is_empty() || arch.is_some_and(str::is_empty) {
                return Err(err("empty package name or architecture"));
            }
            if version.is_empty() {
                return Err(err("empty version"));
            }
            let installed_size_kib = match size.trim() {
                "" => None,
                size => Some(
                    size.parse()
                        .map_err(|_| err(&format!("bad installed size `{size}`")))?,
                ),
            };
            let package = Package {
                name: name.to_owned(),
                arch: arch.map(str::to_owned),
                version: version.to_owned(),
                source: Some(source.trim())
                    .filter(|source| !source.is_empty())
                    .map(str::to_owned),
                installed_size_kib,
            };
            if packages.insert(package.key(), package).is_some() {
                return Err(err("duplicate package"));
            }
        }
        Ok(Self { packages })
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// By `name` or `name:arch`.
    pub fn get(&self, key: &str) -> Option<&Package> {
        self.packages.get(key)
    }

    /// Packages in `name[:arch]` order.
    pub fn iter(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }

    /// Sum of the installed sizes Valve lists.
    pub fn installed_size_kib(&self) -> u64 {
        self.iter().filter_map(|p| p.installed_size_kib).sum()
    }

    /// Packages whose name contains `query`, ignoring ASCII case.
    pub fn search<'a>(&'a self, query: &str) -> impl Iterator<Item = &'a Package> + 'a {
        let query = query.to_ascii_lowercase();
        self.iter()
            .filter(move |p| p.name.to_ascii_lowercase().contains(&query))
    }

    /// What changes going from `self` to `new`.
    pub fn diff(&self, new: &Manifest) -> Diff {
        let mut diff = Diff::default();
        for (key, old) in &self.packages {
            match new.packages.get(key) {
                None => diff.removed.push(old.clone()),
                Some(new) if new.version != old.version => diff.changed.push(Change {
                    name: old.name.clone(),
                    arch: old.arch.clone(),
                    old: old.version.clone(),
                    new: new.version.clone(),
                }),
                Some(_) => {}
            }
        }
        diff.added = new
            .packages
            .iter()
            .filter(|(key, _)| !self.packages.contains_key(*key))
            .map(|(_, package)| package.clone())
            .collect();
        diff
    }
}

/// A package whose version differs between two builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub arch: Option<String>,
    pub old: String,
    pub new: String,
}

/// Packages added, removed and changed between two builds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub added: Vec<Package>,
    pub removed: Vec<Package>,
    pub changed: Vec<Change>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// One line per package name: architectures that changed the same way
    /// are merged, so `libsdl3-0` shows once, not once per arch.
    pub fn summary(&self) -> Vec<SummaryLine> {
        let mut lines: BTreeMap<(String, String), SummaryLine> = BTreeMap::new();
        let mut add = |name: &str, arch: Option<&str>, kind: SummaryKind| {
            let key = (name.to_owned(), kind.to_string());
            let line = lines.entry(key).or_insert_with(|| SummaryLine {
                name: name.to_owned(),
                archs: Vec::new(),
                kind,
            });
            if let Some(arch) = arch {
                line.archs.push(arch.to_owned());
            }
        };
        for change in &self.changed {
            add(
                &change.name,
                change.arch.as_deref(),
                SummaryKind::Changed {
                    old: change.old.clone(),
                    new: change.new.clone(),
                },
            );
        }
        for package in &self.added {
            add(
                &package.name,
                package.arch.as_deref(),
                SummaryKind::Added(package.version.clone()),
            );
        }
        for package in &self.removed {
            add(
                &package.name,
                package.arch.as_deref(),
                SummaryKind::Removed(package.version.clone()),
            );
        }
        lines.into_values().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryKind {
    Added(String),
    Removed(String),
    Changed { old: String, new: String },
}

impl fmt::Display for SummaryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Added(version) => write!(f, "(new) {version}"),
            Self::Removed(version) => write!(f, "{version} (removed)"),
            Self::Changed { old, new } => write!(f, "{old} -> {new}"),
        }
    }
}

/// `libsdl3-0 (amd64, i386)  3.4.18 -> 3.4.20`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryLine {
    pub name: String,
    pub archs: Vec<String>,
    pub kind: SummaryKind,
}

impl fmt::Display for SummaryLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if !self.archs.is_empty() {
            write!(f, " ({})", self.archs.join(", "))?;
        }
        write!(f, "  {}", self.kind)
    }
}
