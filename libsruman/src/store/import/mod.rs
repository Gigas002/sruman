//! `-U <dir>`: adopt a runtime directory that's already on disk, such as
//! Steam's `steamapps/common/SteamLinuxRuntime_sniper` or umu's
//! `steamrt3`, by copying it into the store or linking it in place.

use std::{
    fs, io,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use super::{BuildMeta, Installed, MakeCurrent, Source, Store, StoreError, StoreLock, io_err};
use crate::{
    catalog::{Catalog, Entry},
    version::BuildId,
};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(
        "{0} doesn't look like a Steam Linux Runtime: expected `run` and a `*_platform_*` directory, \
         `scout-on-soldier-entry-point-v2`, or `run.sh` with `version.txt`"
    )]
    NotARuntime(PathBuf),
    #[error("{dir}: can't tell the build id: {message}")]
    NoBuildId { dir: PathBuf, message: String },
    #[error("{dir} holds `{found}`, not `{expected}`")]
    WrongRuntime {
        dir: PathBuf,
        found: String,
        expected: String,
    },
    #[error("{dir} holds `{name}`, which isn't in the runtime catalog")]
    UnknownRuntime { dir: PathBuf, name: String },
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What a directory holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub name: String,
    pub build: BuildId,
    pub entry: Entry,
}

/// Copy into the store, or link in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Copy,
    Link,
}

/// Works out which runtime and build `dir` holds:
///
/// - `scout-on-soldier-entry-point-v2` → `scout`, build from `VERSIONS.txt`;
/// - `run` + `<name>_platform_<build>/` → `<name>`, build from
///   `VERSIONS.txt` (`depot` line), else the platform directory's suffix;
/// - `run.sh` + `version.txt` (`steam-runtime_<build>`) → `scout-ldlp`.
pub fn detect(dir: &Path, catalog: &Catalog) -> Result<Detected, ImportError> {
    let has = |name: &str| dir.join(name).is_file();
    let (name, entry, build) = if has(Entry::ScoutOnSoldier.marker()) {
        ("scout".to_owned(), Entry::ScoutOnSoldier, depot_build(dir)?)
    } else if has(Entry::Container.marker()) {
        let (name, platform_build) =
            platform_dir(dir).ok_or_else(|| ImportError::NotARuntime(dir.to_path_buf()))?;
        let build = match depot_build(dir) {
            Ok(build) => build,
            Err(_) => platform_build,
        };
        (name, Entry::Container, build)
    } else if has(Entry::Ldlp.marker()) && has("version.txt") {
        let text = read(dir, "version.txt")?;
        let build = text
            .trim()
            .strip_prefix("steam-runtime_")
            .and_then(|b| b.parse().ok())
            .ok_or_else(|| ImportError::NoBuildId {
                dir: dir.to_path_buf(),
                message: format!("version.txt says `{}`", text.trim()),
            })?;
        ("scout-ldlp".to_owned(), Entry::Ldlp, build)
    } else {
        return Err(ImportError::NotARuntime(dir.to_path_buf()));
    };
    match catalog.get(&name) {
        Some(def) if def.entry == entry => Ok(Detected { name, build, entry }),
        _ => Err(ImportError::UnknownRuntime {
            dir: dir.to_path_buf(),
            name,
        }),
    }
}

fn read(dir: &Path, file: &str) -> Result<String, ImportError> {
    let path = dir.join(file);
    fs::read_to_string(&path).map_err(|e| ImportError::NoBuildId {
        dir: dir.to_path_buf(),
        message: format!("{file}: {e}"),
    })
}

/// `depot<TAB><build>` in `VERSIONS.txt`.
fn depot_build(dir: &Path) -> Result<BuildId, ImportError> {
    let text = read(dir, "VERSIONS.txt")?;
    text.lines()
        .find_map(|line| {
            let mut fields = line.split('\t');
            (fields.next() == Some("depot")).then(|| fields.next())?
        })
        .and_then(|build| build.trim().parse().ok())
        .ok_or_else(|| ImportError::NoBuildId {
            dir: dir.to_path_buf(),
            message: "VERSIONS.txt has no `depot` line".into(),
        })
}

/// The newest `<name>_platform_<build>/` directory.
fn platform_dir(dir: &Path) -> Option<(String, BuildId)> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let (name, build) = file_name.split_once("_platform_")?;
            Some((name.to_owned(), build.parse::<BuildId>().ok()?))
        })
        .max_by(|a, b| a.1.cmp(&b.1))
}

/// Adopts `dir` into `store`. With `expected`, the directory must hold that
/// runtime.
pub fn import(
    store: &Store,
    lock: &StoreLock,
    catalog: &Catalog,
    dir: &Path,
    expected: Option<&str>,
    mode: ImportMode,
) -> Result<Installed, ImportError> {
    let dir = fs::canonicalize(dir)
        .map_err(io_err(dir))
        .map_err(ImportError::Store)?;
    let found = detect(&dir, catalog)?;
    if let Some(expected) = expected
        && expected != found.name
    {
        return Err(ImportError::WrongRuntime {
            dir,
            found: found.name,
            expected: expected.to_owned(),
        });
    }
    let installed = match mode {
        ImportMode::Link => {
            let meta = BuildMeta::new(
                &found.name,
                &found.build,
                Source::Link {
                    target: dir.clone(),
                },
            );
            store.link(lock, &dir, meta, MakeCurrent::IfNewer)?
        }
        ImportMode::Copy => {
            if store
                .resolve(&crate::version::RuntimeRef {
                    name: found.name.clone(),
                    build: Some(found.build.clone()),
                })
                .is_ok()
            {
                return Err(StoreError::AlreadyInstalled {
                    name: found.name,
                    build: found.build,
                }
                .into());
            }
            let staged = store.staging_dir(lock, &format!("import-{}", found.name))?;
            let tree = staged.join("build");
            copy_tree(&dir, &tree, true)?;
            let meta = BuildMeta::new(
                &found.name,
                &found.build,
                Source::Import { from: dir.clone() },
            );
            store.commit(lock, &tree, meta, MakeCurrent::IfNewer)?
        }
    };
    Ok(installed)
}

/// Copies a runtime tree: regular files (with permissions), directories and
/// symlinks (as links). The top-level `var/` (pressure-vessel's scratch
/// space) is skipped; other file types are refused.
fn copy_tree(from: &Path, to: &Path, top: bool) -> Result<(), StoreError> {
    let meta = fs::symlink_metadata(from).map_err(io_err(from))?;
    fs::create_dir(to).map_err(io_err(to))?;
    for entry in fs::read_dir(from).map_err(io_err(from))? {
        let entry = entry.map_err(io_err(from))?;
        if top && entry.file_name() == "var" {
            continue;
        }
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry.file_type().map_err(io_err(&src))?;
        if kind.is_symlink() {
            let target = fs::read_link(&src).map_err(io_err(&src))?;
            symlink(&target, &dst).map_err(io_err(&dst))?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst, false)?;
        } else if kind.is_file() {
            fs::copy(&src, &dst).map_err(io_err(&dst))?;
        } else {
            return Err(io_err(&src)(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a regular file, directory or symlink",
            )));
        }
    }
    let mode = meta.permissions().mode() | 0o700;
    fs::set_permissions(to, fs::Permissions::from_mode(mode)).map_err(io_err(to))
}
