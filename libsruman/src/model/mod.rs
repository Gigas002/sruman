//! Games and profiles as stored in `games.toml` and `profiles/*.toml`,
//! plus validation.
//!
//! The shapes follow lootrs: a sruman profile is a lootrs `native` profile
//! plus an `[slr]` section ([`SlrSection`], the part lootrs embeds).
//! Validation never fails fast: it returns every [`Issue`] it finds, so
//! `-Pk`/`-Gk` show them all at once.

use std::{
    collections::BTreeMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

mod profile;

pub use profile::{
    LoadedProfiles, Profile, ShaderCache, SlrSection, implicit_profile, load_dir, save_profile,
};

use crate::{catalog::Catalog, paths};

#[cfg(test)]
mod tests;

/// Values validation depends on.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext<'a> {
    /// Used to expand `~` in paths.
    pub home: &'a Path,
    pub catalog: &'a Catalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

/// What an [`Issue`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Game(String),
    Profile(String),
    /// A file that could not be loaded at all.
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub subject: Subject,
    pub message: String,
}

impl Issue {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Game(name) => write!(f, "game `{name}`"),
            Self::Profile(name) => write!(f, "profile `{name}`"),
            Self::File(path) => write!(f, "{}", path.display()),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.severity, self.subject, self.message)
    }
}

/// Checks a game or profile name: `[A-Za-z0-9._-]+`, not `.`/`..`, not
/// starting with `-` (lootrs' rule).
///
/// Names are used as CLI arguments, file names and process tags.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    if name == "." || name == ".." || name.starts_with('-') {
        return Err(format!("name `{name}` is not allowed"));
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    {
        return Err(format!(
            "name `{name}` contains `{c}`; use only letters, digits, `.`, `_` and `-`"
        ));
    }
    Ok(())
}

/// Collects issues for one subject.
pub(crate) struct Issues {
    subject: Subject,
    list: Vec<Issue>,
}

impl Issues {
    pub(crate) fn new(subject: Subject) -> Self {
        Self {
            subject,
            list: Vec::new(),
        }
    }

    fn push(&mut self, severity: Severity, message: impl Into<String>) {
        self.list.push(Issue {
            severity,
            subject: self.subject.clone(),
            message: message.into(),
        });
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.push(Severity::Error, message);
    }

    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        self.push(Severity::Warning, message);
    }

    pub(crate) fn name(&mut self, name: &str) {
        if let Err(message) = validate_name(name) {
            self.error(message);
        }
    }

    /// Errors unless `path` is absolute after `~` expansion.
    pub(crate) fn absolute(&mut self, field: &str, path: &Path, home: &Path) {
        if !paths::expand_tilde(path, home).is_absolute() {
            self.error(format!(
                "`{field}` must be an absolute path (or start with `~`), got `{}`",
                path.display()
            ));
        }
    }

    pub(crate) fn env(&mut self, env: &BTreeMap<String, String>) {
        for key in env.keys() {
            if key.is_empty() || key.contains(['=', '\0']) {
                self.error(format!("`env` key `{key}` is not a valid variable name"));
            }
        }
    }

    pub(crate) fn wrappers(&mut self, wrappers: &[String]) {
        if wrappers.iter().any(|wrapper| wrapper.trim().is_empty()) {
            self.error("`wrappers` must not contain empty entries");
        }
    }

    pub(crate) fn finish(self) -> Vec<Issue> {
        self.list
    }
}

/// Warns about keys a file has that the schema doesn't know, instead of
/// rejecting the file. `known` lists dotted paths (`slr.runtime`); a table
/// key ending in `.*` accepts anything below it (`env.*`).
pub(crate) fn unknown_keys(value: &toml::Value, known: &[&str], issues: &mut Issues) {
    fn walk(value: &toml::Value, prefix: &str, known: &[&str], issues: &mut Issues) {
        let toml::Value::Table(table) = value else {
            return;
        };
        for (key, child) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if known.contains(&format!("{path}.*").as_str()) {
                continue;
            }
            if !known.contains(&path.as_str()) {
                issues.warn(format!("unknown key `{path}` is ignored"));
            } else if child.is_table() {
                walk(child, &path, known, issues);
            }
        }
    }
    walk(value, "", known, issues);
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {message}")]
    Serialize { path: PathBuf, message: String },
    #[error("invalid profile name: {0}")]
    InvalidName(String),
}

/// Writes through a temporary file and a rename.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<(), ModelError> {
    let io_err = |source| ModelError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err)?;
    }
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    fs::write(&tmp, text).map_err(io_err)?;
    fs::rename(&tmp, path).map_err(io_err)
}
