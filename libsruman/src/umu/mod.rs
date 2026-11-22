//! Runtimes for umu (D15): umu-launcher's folder layout over sruman's
//! builds, so umu never downloads or updates a runtime itself.
//!
//! umu looks for its runtime in `$UMU_FOLDERS_PATH/umu/<variant>/` and
//! takes it as installed when that folder has `.installed.ok`. A view here
//! is that folder made of symlinks to the top-level entries of one
//! installed build, plus umu's `umu -> _v2-entry-point` link and the
//! marker; the build itself stays unmodified (`_v2-entry-point` resolves
//! itself, so it runs from the real build, and pressure-vessel's `var/`
//! lands there too). With `UMU_RUNTIME_UPDATE=0` umu leaves the folder
//! alone.
//!
//! A view is swapped atomically (`renameat2(RENAME_EXCHANGE)`) when it
//! moves to another build, and never while a game runs on its build.

use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use rustix::fs::{CWD, RenameFlags, renameat_with};

use crate::{
    store::{Installed, Store, StoreError},
    version::{BuildId, RuntimeRef},
};

#[cfg(test)]
mod tests;

/// umu's install marker (`umu_util.INSTALL_MARKER`).
pub const MARKER: &str = ".installed.ok";
/// Which build a view shows: `<runtime>\t<build>`.
const INFO: &str = ".sruman-view";

/// umu's folder name for a Steam runtime app id (`umu_runtime.py`'s
/// `RUNTIME_VERSIONS`).
pub fn variant(app_id: u32) -> Option<&'static str> {
    match app_id {
        1_391_110 => Some("steamrt2"),
        1_628_350 => Some("steamrt3"),
        4_183_110 => Some("steamrt4"),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UmuError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> UmuError + '_ {
    move |source| UmuError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The result of [`Folders::provide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provided {
    /// `<folders>/umu/<variant>`.
    pub dir: PathBuf,
    /// The build the view shows: the one asked for, or with `kept` the one
    /// a running game still uses.
    pub build: BuildId,
    /// The view was left on another build because a game runs on it.
    pub kept: bool,
    /// The view was created or moved to another build.
    pub changed: bool,
}

/// `UMU_FOLDERS_PATH`: sruman's data directory.
#[derive(Debug, Clone)]
pub struct Folders {
    root: PathBuf,
}

impl Folders {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The value for `UMU_FOLDERS_PATH`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/umu/<variant>`.
    pub fn view_dir(&self, variant: &str) -> PathBuf {
        self.root.join("umu").join(variant)
    }

    /// The runtime and build a view shows, if it is sruman's.
    pub fn shown(&self, variant: &str) -> Option<(String, BuildId)> {
        let text = fs::read_to_string(self.view_dir(variant).join(INFO)).ok()?;
        let (name, build) = text.trim_end().split_once('\t')?;
        Some((name.to_owned(), build.parse().ok()?))
    }

    /// Makes `<root>/umu/<variant>` a view of `build`. A view already on
    /// it is checked and kept; a view on another build that a game runs on
    /// is left alone ([`Provided::kept`]); anything else is replaced under
    /// the store lock.
    pub fn provide(
        &self,
        store: &Store,
        build: &Installed,
        variant: &str,
    ) -> Result<Provided, UmuError> {
        let dir = self.view_dir(variant);
        let provided = |build: &BuildId, kept, changed| Provided {
            dir: dir.clone(),
            build: build.clone(),
            kept,
            changed,
        };
        let shown = || {
            let (name, shown) = self.shown(variant)?;
            store
                .resolve(&RuntimeRef {
                    name,
                    build: Some(shown),
                })
                .ok()
        };
        // Up to date: no lock needed, so launches don't wait on installs.
        if shown().is_some_and(|old| old == *build) && intact(&dir) {
            return Ok(provided(&build.build, false, false));
        }
        // Nothing can remove a build or start a launch's hold check while
        // the lock is held.
        let _lock = store.lock()?;
        if let Some(old) = shown()
            && old.build != build.build
            && old.name == build.name
            && store.in_use(&old)
        {
            return Ok(provided(&old.build, true, false));
        }
        let parent = self.root.join("umu");
        fs::create_dir_all(&parent).map_err(io_err(&parent))?;
        let staging = parent.join(format!(".{variant}.{}", std::process::id()));
        remove(&staging)?;
        build_view(&staging, build)?;
        if dir.symlink_metadata().is_ok() {
            renameat_with(CWD, &staging, CWD, &dir, RenameFlags::EXCHANGE)
                .map_err(|e| io_err(&dir)(e.into()))?;
            // `staging` now holds the old view: symlinks only, or a copy
            // umu downloaded itself before sruman provided one.
            remove(&staging)?;
        } else {
            fs::rename(&staging, &dir).map_err(io_err(&dir))?;
        }
        Ok(provided(&build.build, false, true))
    }
}

/// The view's entry point still leads to a build.
fn intact(dir: &Path) -> bool {
    dir.join(MARKER).is_file() && dir.join("umu").is_file() && dir.join("run").is_file()
}

fn build_view(view: &Path, build: &Installed) -> Result<(), UmuError> {
    fs::create_dir(view).map_err(io_err(view))?;
    let entries = fs::read_dir(&build.dir).map_err(io_err(&build.dir))?;
    for entry in entries {
        let entry = entry.map_err(io_err(&build.dir))?;
        let name = entry.file_name();
        if name == "umu" || name == MARKER || name == INFO {
            continue;
        }
        let link = view.join(&name);
        symlink(build.dir.join(&name), &link).map_err(io_err(&link))?;
    }
    let umu = view.join("umu");
    symlink("_v2-entry-point", &umu).map_err(io_err(&umu))?;
    let info = view.join(INFO);
    fs::write(&info, format!("{}\t{}\n", build.name, build.build)).map_err(io_err(&info))?;
    // Last: umu takes a folder with the marker as complete.
    let marker = view.join(MARKER);
    fs::write(&marker, "ok\n").map_err(io_err(&marker))
}

fn remove(path: &Path) -> Result<(), UmuError> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_err(path)(e)),
        _ => Ok(()),
    }
}
