//! Prefixes: a directory whose `home/` a game gets as its private `$HOME`,
//! like a wine prefix's `drive_c/users`.
//!
//! A prefix is any directory a profile names, and several profiles may share
//! one. [`ensure`] runs on every launch (lootrs' pattern): it creates the
//! skeleton, records the use in `<prefix>/.sruman.toml`, and returns a shared
//! [`PrefixLock`] that the launcher keeps for as long as the game runs.
//! [`reset`] and [`remove`] need that lock exclusively, so they fail with
//! [`PrefixError::Busy`] while any game runs in the prefix.
//!
//! Lock files live outside the prefix, in the caller's lock directory
//! (`$XDG_RUNTIME_DIR/sruman`), named by a hash of the prefix's canonical
//! path. The hash is SHA-256, so every program that links libsruman (sruman,
//! lootrs, gaki) agrees on the name.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    os::fd::{AsFd, BorrowedFd},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{model::LoadedProfiles, paths, store::make_dirs_writable};

#[cfg(test)]
mod tests;

/// sruman's record of a prefix, at `<prefix>/.sruman.toml`.
pub const STATE_FILE: &str = ".sruman.toml";
/// The private home, at `<prefix>/home`.
pub const HOME_DIR: &str = "home";
/// Created in the home so XDG-aware games and the env-only home mode find
/// their directories (relative to the home).
pub const SKELETON: [&str; 4] = [".cache", ".config", ".local/share", ".local/state"];

/// How a runtime gives the game its prefix home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HomeMode {
    /// pressure-vessel's private home (`PRESSURE_VESSEL_HOME`): `$HOME` and
    /// the passwd entry both point at it.
    Container,
    /// No container (`scout-ldlp`): only `HOME` and `XDG_*` are redirected.
    Env,
}

/// What [`ensure`] records about one use of the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUse {
    /// The runtime build the game ran on.
    pub build: String,
    pub profile: String,
    pub home: HomeMode,
}

/// The contents of `.sruman.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixState {
    /// sruman version that last wrote the state.
    #[serde(default)]
    pub sruman: String,
    /// By runtime name: the last launch with that runtime.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub runtimes: BTreeMap<String, LastUse>,
}

/// One launch's use of a prefix, for [`ensure`].
#[derive(Debug, Clone, Copy)]
pub struct PrefixUse<'a> {
    pub runtime: &'a str,
    pub build: &'a str,
    pub profile: &'a str,
    pub home: HomeMode,
}

/// Values that come from settings, not the profile.
#[derive(Debug, Clone, Copy)]
pub struct PrefixContext<'a> {
    /// Directory for the lock files.
    pub lock_dir: &'a Path,
    /// The user's real home: a prefix may not be it or contain it.
    pub home: &'a Path,
}

/// A lock on one prefix: shared while games run, exclusive for
/// [`reset`]/[`remove`]. Released on drop.
#[derive(Debug)]
pub struct PrefixLock {
    file: File,
    prefix: PathBuf,
}

impl PrefixLock {
    /// The canonical prefix path the lock is keyed by.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// Pass this to the child (without `CLOEXEC`) so the lock outlives a
    /// detached launcher.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

/// What [`ensure`] did.
#[derive(Debug)]
pub struct Prepared {
    /// Keep this until the game exits.
    pub lock: PrefixLock,
    /// `<prefix>/home`.
    pub home: PathBuf,
    /// The prefix directory didn't exist before.
    pub created: bool,
    /// The state after this launch was recorded.
    pub state: PrefixState,
}

#[derive(Debug, thiserror::Error)]
pub enum PrefixError {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("prefix {} is in use; stop the games running in it first", .0.display())]
    Busy(PathBuf),
    #[error("prefix {} must be an absolute path", .0.display())]
    Relative(PathBuf),
    #[error("prefix {} is or contains your home directory; pick a directory of its own", .0.display())]
    ContainsHome(PathBuf),
    #[error("{} is not a sruman prefix (no {STATE_FILE}); not deleting it", .0.display())]
    NotAPrefix(PathBuf),
}

impl PrefixError {
    /// Exit code 4 territory: something is running.
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Busy(_))
    }
}

fn io_err(context: impl Into<String>) -> impl FnOnce(io::Error) -> PrefixError {
    let context = context.into();
    move |source| PrefixError::Io { context, source }
}

/// The prefix as locks and comparisons see it: canonical when it exists,
/// otherwise as given (made absolute is the caller's job).
pub fn canonical(prefix: &Path) -> PathBuf {
    fs::canonicalize(prefix).unwrap_or_else(|_| prefix.to_path_buf())
}

/// The lock file for `prefix` in `lock_dir`.
pub fn lock_path(lock_dir: &Path, prefix: &Path) -> PathBuf {
    let digest = Sha256::digest(canonical(prefix).as_os_str().as_encoded_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    lock_dir.join(format!("prefix-{hex}.lock"))
}

/// Takes the prefix's lock without waiting; [`PrefixError::Busy`] if it
/// conflicts with another holder.
pub fn lock(lock_dir: &Path, prefix: &Path, exclusive: bool) -> Result<PrefixLock, PrefixError> {
    let path = lock_path(lock_dir, prefix);
    let context = || format!("locking {}", path.display());
    fs::create_dir_all(lock_dir).map_err(io_err(context()))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io_err(context()))?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match result {
        Ok(()) => Ok(PrefixLock {
            file,
            prefix: canonical(prefix),
        }),
        Err(fs::TryLockError::WouldBlock) => Err(PrefixError::Busy(prefix.to_path_buf())),
        Err(fs::TryLockError::Error(e)) => Err(io_err(context())(e)),
    }
}

/// Whether a game holds the prefix.
pub fn in_use(lock_dir: &Path, prefix: &Path) -> bool {
    let path = lock_path(lock_dir, prefix);
    // No lock file: nobody ever locked it since boot.
    let Ok(file) = File::open(&path) else {
        return false;
    };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
}

fn check_location(prefix: &Path, home: &Path) -> Result<(), PrefixError> {
    if !prefix.is_absolute() {
        return Err(PrefixError::Relative(prefix.to_path_buf()));
    }
    let (prefix_c, home_c) = (canonical(prefix), canonical(home));
    if home_c.starts_with(&prefix_c) {
        return Err(PrefixError::ContainsHome(prefix.to_path_buf()));
    }
    Ok(())
}

/// Creates the prefix and its home skeleton if needed, takes the shared
/// lock, and records `usage` in the state file. `prefix` is absolute (`~`
/// already expanded).
pub fn ensure(
    prefix: &Path,
    usage: &PrefixUse,
    ctx: &PrefixContext,
) -> Result<Prepared, PrefixError> {
    check_location(prefix, ctx.home)?;
    let created = !prefix.exists();
    let home = prefix.join(HOME_DIR);
    fs::create_dir_all(&home).map_err(io_err(format!("creating {}", home.display())))?;
    // Lock after creating, so the key is the canonical path from the start.
    let lock = lock(ctx.lock_dir, prefix, false)?;
    for dir in SKELETON {
        let dir = home.join(dir);
        fs::create_dir_all(&dir).map_err(io_err(format!("creating {}", dir.display())))?;
    }

    let mut state = read_state(prefix);
    let entry = LastUse {
        build: usage.build.to_owned(),
        profile: usage.profile.to_owned(),
        home: usage.home,
    };
    if state.runtimes.get(usage.runtime) != Some(&entry) || state.sruman != crate::VERSION {
        state.runtimes.insert(usage.runtime.to_owned(), entry);
        state.sruman = crate::VERSION.to_owned();
        write_state(prefix, &state)?;
    }
    Ok(Prepared {
        lock,
        home,
        created,
        state,
    })
}

/// The state file, or an empty state if there is none (or it's unreadable).
pub fn read_state(prefix: &Path) -> PrefixState {
    fs::read_to_string(prefix.join(STATE_FILE))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_state(prefix: &Path, state: &PrefixState) -> Result<(), PrefixError> {
    let path = prefix.join(STATE_FILE);
    let context = || format!("writing {}", path.display());
    let text = toml::to_string(state).map_err(|e| io_err(context())(io::Error::other(e)))?;
    // Concurrent launches share the lock, so each writes its own temp file.
    let tmp = prefix.join(format!("{STATE_FILE}.tmp.{}", std::process::id()));
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, &path))
        .map_err(io_err(context()))
}

fn remove_dir(path: &Path) -> Result<(), PrefixError> {
    let context = || format!("removing {}", path.display());
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            make_dirs_writable(path);
            fs::remove_dir_all(path).map_err(io_err(context()))
        }
        Err(e) => Err(io_err(context())(e)),
    }
}

/// Empties the prefix's home (saves and configs!) and forgets its state;
/// the next [`ensure`] starts fresh. Fails while a game runs in it.
pub fn reset(prefix: &Path, ctx: &PrefixContext) -> Result<(), PrefixError> {
    check_location(prefix, ctx.home)?;
    let _lock = lock(ctx.lock_dir, prefix, true)?;
    remove_dir(&prefix.join(HOME_DIR))?;
    match fs::remove_file(prefix.join(STATE_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_err(format!(
            "removing {}",
            prefix.join(STATE_FILE).display()
        ))(e)),
        _ => Ok(()),
    }
}

/// Deletes the whole prefix directory (`-Pdd`). Only a directory with a
/// state file counts as a prefix, so a mistyped path can't take a real
/// directory with it. Fails while a game runs in it.
pub fn remove(prefix: &Path, ctx: &PrefixContext) -> Result<(), PrefixError> {
    check_location(prefix, ctx.home)?;
    if !prefix.exists() {
        return Ok(());
    }
    if !prefix.join(STATE_FILE).is_file() {
        return Err(PrefixError::NotAPrefix(prefix.to_path_buf()));
    }
    let _lock = lock(ctx.lock_dir, prefix, true)?;
    remove_dir(prefix)
}

/// Names of the profiles whose prefix is `prefix` (compared after `~`
/// expansion and canonicalization), sorted.
pub fn sharing<'a>(prefix: &Path, profiles: &'a LoadedProfiles, home: &Path) -> Vec<&'a str> {
    let target = canonical(&paths::expand_tilde(prefix, home));
    profiles
        .profiles
        .values()
        .filter(|profile| {
            profile
                .prefix
                .as_deref()
                .is_some_and(|p| canonical(&paths::expand_tilde(p, home)) == target)
        })
        .map(|profile| profile.name.as_str())
        .collect()
}
