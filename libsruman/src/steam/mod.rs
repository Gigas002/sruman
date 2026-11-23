//! Steam's own runtimes, replaced by links into the store (D17).
//!
//! Steam keeps a runtime for its client
//! (`<steam>/steamrt64/pv-runtime/steam-runtime-steamrt`, [`CLIENT_RUNTIME`])
//! and installs the game runtimes as apps in its libraries
//! (`steamapps/common/SteamLinuxRuntime_4`, …). It checks its client files
//! at every start and restores a missing runtime, so sruman doesn't delete
//! them: [`Steam::link`] replaces each with a symlink to the same build in
//! the store, and Steam follows the link and finds what it expects.
//!
//! A link names one build, never `current`, so `upgrade` doesn't move a
//! runtime under Steam, and `clean` and `remove` keep linked builds (the
//! caller asks [`Steam::linked`]). When Steam updates a runtime, it writes
//! the new build through the link into the old one: [`State::Changed`]
//! shows that, and linking again imports the new build and repoints the
//! link.

use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use rustix::fs::{CWD, RenameFlags, renameat_with};

use crate::{
    catalog::Catalog,
    mtree::Depth,
    store::{
        Installed, Store, StoreError,
        import::{self, ImportError, ImportMode},
    },
    version::{BuildId, RuntimeRef},
};

#[cfg(test)]
mod tests;

/// The runtime Steam's 64-bit client runs in.
pub const CLIENT_RUNTIME: &str = "steamrt3c";

/// Where the client's runtime lives under Steam's root.
const CLIENT_DIR: &str = "steamrt64/pv-runtime/steam-runtime-steamrt";

/// The game runtimes Steam installs as apps, by their folder in
/// `steamapps/common`.
pub const GAME_FOLDERS: [(&str, &str); 4] = [
    ("scout", "SteamLinuxRuntime"),
    ("soldier", "SteamLinuxRuntime_soldier"),
    ("sniper", "SteamLinuxRuntime_sniper"),
    ("steamrt4", "SteamLinuxRuntime_4"),
];

/// Errors from linking Steam's runtimes.
#[derive(Debug, thiserror::Error)]
pub enum SteamError {
    /// The runtime store failed.
    #[error(transparent)]
    Store(#[from] StoreError),

    /// Taking over Steam's copy failed.
    #[error(transparent)]
    Import(#[from] ImportError),

    /// A filesystem operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory it failed on.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },

    /// Steam is running: it may be checking or updating its files.
    #[error("Steam is running; quit it first")]
    Running,

    /// A game runs on the runtime.
    #[error("{} is in use by a running game; stop it first", .0.display())]
    InUse(PathBuf),

    /// The slot is a symlink that doesn't lead to a build in the store.
    #[error(
        "{} links to {}, not to a build sruman has; move it away first",
        path.display(),
        target.display()
    )]
    Foreign {
        /// Steam's runtime folder.
        path: PathBuf,
        /// Where it links to.
        target: PathBuf,
    },

    /// The build to link doesn't match its file list.
    #[error(
        "{name} {build} doesn't match its file list ({count} problems; `sruman verify {name}`); {} is left as it is",
        path.display()
    )]
    Damaged {
        /// The runtime.
        name: String,
        /// The build.
        build: BuildId,
        /// How many files don't match.
        count: usize,
        /// Steam's runtime folder.
        path: PathBuf,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SteamError + '_ {
    move |source| SteamError::Io {
        path: path.to_owned(),
        source,
    }
}

/// One place Steam keeps a runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The runtime it holds: `steamrt3c`, `steamrt4`, …
    pub runtime: String,
    /// Steam's folder (or sruman's link in its place).
    pub path: PathBuf,
    /// The client's runtime, not a game runtime.
    pub client: bool,
}

/// What a [`Slot`] holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Steam's own copy, with its build when `VERSIONS.txt` names one.
    Copy(Option<BuildId>),
    /// A link to this build.
    Linked(Installed),
    /// A link to this build, which Steam has since updated in place to
    /// `now` (`None`: `VERSIONS.txt` names no build any more).
    Changed {
        /// The build the link leads to.
        build: Installed,
        /// The build its files are now.
        now: Option<BuildId>,
    },
    /// A link to something that isn't a build in the store, or to nothing.
    Foreign(PathBuf),
}

impl Slot {
    /// What the slot holds.
    pub fn state(&self, store: &Store) -> Result<State, SteamError> {
        let meta = fs::symlink_metadata(&self.path).map_err(io_err(&self.path))?;
        if !meta.file_type().is_symlink() {
            return Ok(State::Copy(import::depot_build(&self.path).ok()));
        }
        let target = fs::read_link(&self.path).map_err(io_err(&self.path))?;
        let Ok(real) = fs::canonicalize(&self.path) else {
            return Ok(State::Foreign(target));
        };
        for build in store.installed_of(&self.runtime)? {
            if fs::canonicalize(&build.dir).is_ok_and(|dir| dir == real) {
                let now = import::depot_build(&real).ok();
                return Ok(if now.as_ref() == Some(&build.build) {
                    State::Linked(build)
                } else {
                    State::Changed { build, now }
                });
            }
        }
        Ok(State::Foreign(target))
    }
}

/// The result of [`Steam::link`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The build the slot links to now.
    pub build: Installed,
    /// The slot changed: Steam's copy (or an updated build) was replaced.
    pub changed: bool,
    /// The build was taken over from Steam's copy, not already installed.
    pub imported: bool,
    /// The build the slot linked to before, which Steam updated in place:
    /// its files no longer match it.
    pub damaged: Option<Installed>,
}

/// A Steam installation.
#[derive(Debug, Clone)]
pub struct Steam {
    root: PathBuf,
    pid_file: PathBuf,
}

impl Steam {
    /// Finds Steam: `configured` when given, else `~/.steam/root`, else
    /// `$XDG_DATA_HOME/Steam`. `None` when there is no Steam there.
    pub fn locate(configured: Option<&Path>, home: &Path, data_home: &Path) -> Option<Self> {
        let candidates = match configured {
            Some(dir) => vec![dir.to_path_buf()],
            None => vec![home.join(".steam/root"), data_home.join("Steam")],
        };
        let root = candidates
            .into_iter()
            .find(|dir| dir.join("steamapps").is_dir() || dir.join("steamrt64").is_dir())?;
        Some(Self::at(
            fs::canonicalize(&root).unwrap_or(root),
            home.join(".steam/steam.pid"),
        ))
    }

    /// Steam at `root`, with its pid in `pid_file` (`~/.steam/steam.pid`).
    pub fn at(root: impl Into<PathBuf>, pid_file: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            pid_file: pid_file.into(),
        }
    }

    /// Steam's root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Steam's libraries: its root, and every `path` in
    /// `steamapps/libraryfolders.vdf` that has a `steamapps` folder.
    pub fn libraries(&self) -> Vec<PathBuf> {
        let mut out = vec![self.root.clone()];
        let vdf = self.root.join("steamapps/libraryfolders.vdf");
        for path in fs::read_to_string(vdf)
            .map(|text| library_paths(&text))
            .unwrap_or_default()
        {
            if !path.join("steamapps").is_dir() {
                continue;
            }
            let path = fs::canonicalize(&path).unwrap_or(path);
            if !out.contains(&path) {
                out.push(path);
            }
        }
        out
    }

    /// Every runtime folder Steam has (as its own copy or as a link): the
    /// client's, then the game runtimes in each library.
    pub fn slots(&self) -> Vec<Slot> {
        let exists = |path: &Path| fs::symlink_metadata(path).is_ok();
        let mut out = Vec::new();
        let client = self.root.join(CLIENT_DIR);
        if exists(&client) {
            out.push(Slot {
                runtime: CLIENT_RUNTIME.to_owned(),
                path: client,
                client: true,
            });
        }
        for library in self.libraries() {
            for (runtime, folder) in GAME_FOLDERS {
                let path = library.join("steamapps/common").join(folder);
                if exists(&path) {
                    out.push(Slot {
                        runtime: runtime.to_owned(),
                        path,
                        client: false,
                    });
                }
            }
        }
        out
    }

    /// Whether Steam runs: the process in its pid file is called `steam`.
    pub fn running(&self) -> bool {
        let Some(pid) = fs::read_to_string(&self.pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        else {
            return false;
        };
        fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|comm| comm.trim() == "steam")
    }

    /// The builds Steam links to, with their slots: `clean` and `remove`
    /// keep these.
    pub fn linked(&self, store: &Store) -> Vec<(Slot, Installed)> {
        self.slots()
            .into_iter()
            .filter_map(|slot| match slot.state(store).ok()? {
                State::Linked(build) | State::Changed { build, .. } => Some((slot, build)),
                State::Copy(_) | State::Foreign(_) => None,
            })
            .collect()
    }

    /// Replaces the slot's runtime with a link to the same build in the
    /// store: an installed build is used as it is, otherwise Steam's copy
    /// is imported. A link already right is left alone; a link to a build
    /// Steam has updated in place moves to the build it became. The build
    /// is checked against its file list first; on a mismatch nothing
    /// changes.
    pub fn link(
        &self,
        store: &Store,
        catalog: &Catalog,
        slot: &Slot,
    ) -> Result<Linked, SteamError> {
        if self.running() {
            return Err(SteamError::Running);
        }
        let (source, damaged) = match slot.state(store)? {
            State::Linked(build) => {
                // A link through `current` would move with `upgrade`.
                let exact = fs::read_link(&slot.path).is_ok_and(|target| target == build.dir);
                if !exact {
                    swap_in_link(&slot.path, &build.dir)?;
                }
                return Ok(Linked {
                    build,
                    changed: !exact,
                    imported: false,
                    damaged: None,
                });
            }
            State::Foreign(target) => {
                return Err(SteamError::Foreign {
                    path: slot.path.clone(),
                    target,
                });
            }
            State::Copy(_) => (slot.path.clone(), None),
            State::Changed { build, .. } => (
                fs::canonicalize(&build.dir).map_err(io_err(&build.dir))?,
                Some(build),
            ),
        };
        if Store::dir_in_use(&source) {
            return Err(SteamError::InUse(slot.path.clone()));
        }
        let lock = store.lock()?;
        let found = import::detect(&source, catalog)?;
        if found.name != slot.runtime {
            return Err(ImportError::WrongRuntime {
                dir: source,
                found: found.name,
                expected: slot.runtime.clone(),
            }
            .into());
        }
        let wanted = RuntimeRef {
            name: found.name.clone(),
            build: Some(found.build.clone()),
        };
        let damaged_build = |build: &Installed, count| SteamError::Damaged {
            name: build.name.clone(),
            build: build.build.clone(),
            count,
            path: slot.path.clone(),
        };
        let (build, imported) = match store.resolve(&wanted) {
            // The updated build would be imported over itself.
            Ok(build) if damaged.as_ref() == Some(&build) => return Err(damaged_build(&build, 0)),
            Ok(build) => (build, false),
            Err(_) => (
                import::import(
                    store,
                    &lock,
                    catalog,
                    &source,
                    Some(&slot.runtime),
                    ImportMode::Copy,
                )?,
                true,
            ),
        };
        if let Some(report) = store.verify(&build, Depth::Presence)?
            && !report.problems.is_empty()
        {
            return Err(damaged_build(&build, report.problems.len()));
        }
        swap_in_link(&slot.path, &build.dir)?;
        Ok(Linked {
            build,
            changed: true,
            imported,
            damaged,
        })
    }

    /// Puts a copy of the build the slot links to back in its place, so
    /// Steam has its own runtime again. `false` when it already has.
    pub fn unlink(&self, store: &Store, slot: &Slot) -> Result<bool, SteamError> {
        if self.running() {
            return Err(SteamError::Running);
        }
        let build = match slot.state(store)? {
            State::Copy(_) => return Ok(false),
            State::Foreign(target) => {
                return Err(SteamError::Foreign {
                    path: slot.path.clone(),
                    target,
                });
            }
            State::Linked(build) | State::Changed { build, .. } => build,
        };
        if store.in_use(&build) {
            return Err(SteamError::InUse(slot.path.clone()));
        }
        let _lock = store.lock()?;
        let source = fs::canonicalize(&build.dir).map_err(io_err(&build.dir))?;
        let staging = staging_path(&slot.path)?;
        remove_any(&staging)?;
        if let Err(err) = import::copy_runtime(&source, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(err.into());
        }
        renameat_with(CWD, &staging, CWD, &slot.path, RenameFlags::EXCHANGE)
            .map_err(|e| io_err(&slot.path)(e.into()))?;
        // `staging` is the link now.
        fs::remove_file(&staging).map_err(io_err(&staging))?;
        Ok(true)
    }
}

/// Builds of Steam's runtimes whose files are another build now: Steam
/// updated them in place through a link (`VERSIONS.txt` names another
/// build). `sruman steam clean` removes the ones Steam no longer links to.
pub fn rewritten(store: &Store) -> Result<Vec<Installed>, StoreError> {
    let names = std::iter::once(CLIENT_RUNTIME).chain(GAME_FOLDERS.iter().map(|(name, _)| *name));
    let mut out = Vec::new();
    for name in names {
        for build in store.installed_of(name)? {
            let now = fs::canonicalize(&build.dir)
                .ok()
                .and_then(|dir| import::depot_build(&dir).ok());
            if now.is_some_and(|now| now != build.build) {
                out.push(build);
            }
        }
    }
    Ok(out)
}

/// The `path` values of Steam's `libraryfolders.vdf`.
fn library_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("\"path\"")?.trim();
            let value = value.strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(value.replace("\\\\", "\\")))
        })
        .collect()
}

/// `.<name>.sruman-<pid>` next to `path`.
fn staging_path(path: &Path) -> Result<PathBuf, SteamError> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io_err(path)(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a folder in a folder",
        )));
    };
    Ok(parent.join(format!(
        ".{}.sruman-{}",
        name.to_string_lossy(),
        std::process::id()
    )))
}

/// Replaces `path` (Steam's folder, or a link) with a symlink to `target`
/// in one rename; Steam's folder is deleted afterwards.
fn swap_in_link(path: &Path, target: &Path) -> Result<(), SteamError> {
    let staging = staging_path(path)?;
    remove_any(&staging)?;
    symlink(target, &staging).map_err(io_err(&staging))?;
    let is_link = fs::symlink_metadata(path)
        .map_err(io_err(path))?
        .file_type()
        .is_symlink();
    if is_link {
        return fs::rename(&staging, path).map_err(io_err(path));
    }
    renameat_with(CWD, &staging, CWD, path, RenameFlags::EXCHANGE)
        .map_err(|e| io_err(path)(e.into()))?;
    // `staging` holds Steam's folder now.
    remove_any(&staging)
}

fn remove_any(path: &Path) -> Result<(), SteamError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(io_err(path))
}
