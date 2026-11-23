//! Steam's library, caches and logs moved out of its folder (D18), each
//! replaced by a symlink to where it went, so Steam's folder keeps only the
//! client: the library to the folder you name, caches under
//! `$XDG_CACHE_HOME/steam`, logs under `$XDG_STATE_HOME/steam`.

use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use super::{Steam, SteamError, io_err, remove_any, staging_path};
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
    /// folder is missing (a move cut short): then it's only linked.
    pub fn relocate(&self, place: &Place) -> Result<bool, SteamError> {
        if self.running() {
            return Err(SteamError::Running);
        }
        match place.placed() {
            Placed::Moved => return Ok(false),
            Placed::Elsewhere(target) => {
                return Err(SteamError::Foreign {
                    path: place.path.clone(),
                    target,
                });
            }
            Placed::Missing => {
                fs::create_dir_all(&place.target).map_err(io_err(&place.target))?;
            }
            Placed::Here => {
                if !is_empty_or_missing(&place.target)? {
                    return Err(SteamError::TargetExists(place.target.clone()));
                }
                remove_any(&place.target)?;
                if let Some(parent) = place.target.parent() {
                    fs::create_dir_all(parent).map_err(io_err(parent))?;
                }
                move_dir(&place.path, &place.target)?;
            }
        }
        if let Some(parent) = place.path.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        symlink(&place.target, &place.path).map_err(io_err(&place.path))?;
        Ok(true)
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
                return Err(SteamError::Foreign {
                    path: place.path.clone(),
                    target,
                });
            }
            Placed::Moved => {}
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
