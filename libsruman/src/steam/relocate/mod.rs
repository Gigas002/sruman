//! Steam's library, caches and logs moved out of its folder (D18), each
//! replaced by a symlink to where it went, so Steam's folder keeps only the
//! client: the library to the folder you name, caches under
//! `$XDG_CACHE_HOME/steam`, logs under `$XDG_STATE_HOME/steam`.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    path::{Component, Path, PathBuf},
};

use super::{Steam, SteamError, busy, io_err, remove_any, staging_path};
use crate::store::import;

#[cfg(test)]
mod tests;

/// Where [`Steam::places`] sends things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targets {
    /// The library folder `steamapps` moves into; `None` leaves it.
    pub library: Option<PathBuf>,
    /// `$XDG_CACHE_HOME/steam`.
    pub cache: PathBuf,
    /// `$XDG_STATE_HOME/steam`.
    pub state: PathBuf,
}

/// One folder of Steam's and where it belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// What it is: `library`, `appcache`, …
    pub name: &'static str,
    /// The folder under Steam's root.
    pub path: PathBuf,
    /// Where it goes.
    pub target: PathBuf,
}

/// Where a [`Place`] is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placed {
    /// In Steam's folder.
    Here,
    /// Moved: Steam's folder has a link to the target.
    Moved,
    /// Not there yet (Steam makes it when it needs it).
    Missing,
    /// A link somewhere else.
    Elsewhere(PathBuf),
}

/// The caches under Steam's root, by path, and their folder under
/// `$XDG_CACHE_HOME/steam`.
const CACHES: [(&str, &str); 6] = [
    ("appcache", "appcache"),
    ("depotcache", "depotcache"),
    ("package", "package"),
    ("config/htmlcache", "htmlcache"),
    ("config/avatarcache", "avatarcache"),
    ("steamapps/shadercache", "shadercache"),
];

impl Place {
    /// Where the place is now.
    pub fn placed(&self) -> Placed {
        let Ok(meta) = fs::symlink_metadata(&self.path) else {
            return Placed::Missing;
        };
        if !meta.file_type().is_symlink() {
            return Placed::Here;
        }
        let target = fs::read_link(&self.path).unwrap_or_default();
        let same = |a: &Path, b: &Path| match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        if same(&self.path, &self.target) || target == self.target {
            Placed::Moved
        } else {
            Placed::Elsewhere(target)
        }
    }
}

impl Steam {
    /// Steam's folders that belong elsewhere, in the order they move: the
    /// library first, so its shader cache moves from the library's new
    /// place.
    pub fn places(&self, targets: &Targets) -> Vec<Place> {
        let mut out = Vec::new();
        if let Some(library) = &targets.library {
            out.push(Place {
                name: "library",
                path: self.root.join("steamapps"),
                target: library.join("steamapps"),
            });
        }
        for (path, name) in CACHES {
            out.push(Place {
                name,
                path: self.root.join(path),
                target: targets.cache.join(name),
            });
        }
        out.push(Place {
            name: "logs",
            path: self.root.join("logs"),
            target: targets.state.join("logs"),
        });
        out
    }

    /// Moves `place` to its target and leaves a link in its stead; a
    /// missing one gets the link (and an empty target) right away, so Steam
    /// never makes it in its own folder. `false` when it was moved already.
    /// A target that exists and isn't empty is refused, unless Steam's
    /// folder is missing (a move cut short): then it's only linked. So is
    /// a folder a process uses, and a target inside the folder (or the
    /// other way round).
    ///
    /// Steam's folder is private (`0700`) while what's in it isn't, so
    /// folders sruman makes for a target are private, and the moved folder
    /// loses the permissions Steam's folder didn't give.
    pub fn relocate(&self, place: &Place) -> Result<bool, SteamError> {
        if self.running() {
            return Err(SteamError::Running);
        }
        let placed = place.placed();
        if matches!(placed, Placed::Here | Placed::Missing) {
            let (path, target) = (resolve(&place.path), resolve(&place.target));
            if path.starts_with(&target) || target.starts_with(&path) {
                return Err(SteamError::Nested {
                    path: place.path.clone(),
                    target: place.target.clone(),
                });
            }
        }
        match placed {
            Placed::Moved => return Ok(false),
            Placed::Elsewhere(target) => {
                return Err(SteamError::Relocated {
                    path: place.path.clone(),
                    target,
                    wanted: place.target.clone(),
                });
            }
            Placed::Missing => {
                private_dirs(&place.target)?;
            }
            Placed::Here => {
                if !is_empty_or_missing(&place.target)? {
                    return Err(SteamError::TargetExists(place.target.clone()));
                }
                not_busy(&place.path)?;
                remove_any(&place.target)?;
                if let Some(parent) = place.target.parent() {
                    private_dirs(parent)?;
                }
                move_dir(&place.path, &place.target)?;
            }
        }
        self.keep_private(&place.target)?;
        if let Some(parent) = place.path.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        symlink(&place.target, &place.path).map_err(io_err(&place.path))?;
        Ok(true)
    }

    /// Takes from `dir` the permissions Steam's root doesn't give.
    fn keep_private(&self, dir: &Path) -> Result<(), SteamError> {
        let root = fs::metadata(&self.root).map_err(io_err(&self.root))?;
        let denied = 0o077 & !root.permissions().mode();
        let meta = fs::metadata(dir).map_err(io_err(dir))?;
        let mode = meta.permissions().mode();
        if mode & denied == 0 {
            return Ok(());
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(mode & !denied)).map_err(io_err(dir))
    }

    /// Moves `place` back into Steam's folder in place of the link. `false`
    /// when it's there already.
    pub fn unrelocate(&self, place: &Place) -> Result<bool, SteamError> {
        if self.running() {
            return Err(SteamError::Running);
        }
        match place.placed() {
            Placed::Here | Placed::Missing => return Ok(false),
            Placed::Elsewhere(target) => {
                return Err(SteamError::Relocated {
                    path: place.path.clone(),
                    target,
                    wanted: place.target.clone(),
                });
            }
            Placed::Moved => {}
        }
        if fs::symlink_metadata(&place.target).is_ok() {
            not_busy(&place.target)?;
        }
        fs::remove_file(&place.path).map_err(io_err(&place.path))?;
        if fs::symlink_metadata(&place.target).is_ok() {
            move_dir(&place.target, &place.path)?;
        } else {
            fs::create_dir(&place.path).map_err(io_err(&place.path))?;
        }
        Ok(true)
    }
}

/// `path` with its symlinks resolved as far as it exists, and `..` taken
/// out of the rest.
fn resolve(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    let base = loop {
        if let Ok(real) = fs::canonicalize(existing) {
            break real;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => break existing.to_path_buf(),
        }
    };
    let mut out = base;
    for name in rest.iter().rev() {
        out.push(name);
    }
    let mut clean = PathBuf::new();
    for part in out.components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    clean
}

/// Creates `dir` and its missing parents, private (`0700`).
fn private_dirs(dir: &Path) -> Result<(), SteamError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io_err(dir))
}

/// Refuses a folder a process has files open in (a game started without
/// Steam, a shell in it): moving it under the process loses its writes.
fn not_busy(dir: &Path) -> Result<(), SteamError> {
    let real = fs::canonicalize(dir).map_err(io_err(dir))?;
    match busy(&real) {
        Some((pid, name)) => Err(SteamError::Busy {
            path: dir.to_path_buf(),
            pid,
            name,
        }),
        None => Ok(()),
    }
}

fn is_empty_or_missing(dir: &Path) -> Result<bool, SteamError> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_none()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(io_err(dir)(e)),
    }
}

/// Renames `from` to `to`; across filesystems, copies (next to `to`, then
/// renamed into place) and deletes `from`.
fn move_dir(from: &Path, to: &Path) -> Result<(), SteamError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() != io::ErrorKind::CrossesDevices => Err(io_err(to)(e)),
        Err(_) => copy_move(from, to),
    }
}

/// [`move_dir`] across filesystems: a copy next to `to`, renamed into place,
/// then `from` deleted.
fn copy_move(from: &Path, to: &Path) -> Result<(), SteamError> {
    let staging = staging_path(to)?;
    remove_any(&staging)?;
    if let Err(err) = import::copy_tree(from, &staging, &|_| false) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err.into());
    }
    fs::rename(&staging, to).map_err(io_err(to))?;
    remove_any(from)
}
