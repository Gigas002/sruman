//! Installed runtime builds.
//!
//! ```text
//! <root>/
//!   .lock                      exclusive while a sruman process changes the store
//!   .staging/                  partial installs and removals; wiped under the lock
//!   <name>/
//!     <build_id>/              the extracted build (a symlink for a linked one)
//!     <build_id>.toml          BuildMeta: where it came from
//!     <build_id>.lock          held (shared) while sruman runs a game on it
//!     current -> <build_id>    the build `<name>` without a pin resolves to
//! ```
//!
//! Metadata and locks sit next to a build, never inside it, so a linked
//! build (Steam's own copy) is never written to by sruman. A build directory
//! is not modified after install, but it must stay writable: pressure-vessel
//! opens `files/.ref` and writes `var/`.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    mtree::{self, Depth},
    version::{BuildId, RuntimeRef},
};

pub mod import;

#[cfg(test)]
pub(crate) mod tests;

const LOCK_FILE: &str = ".lock";
const STAGING_DIR: &str = ".staging";
const CURRENT: &str = "current";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the runtime store is locked by another sruman process ({0})")]
    Busy(PathBuf),
    #[error("runtime `{name}` is not installed; install it with `sruman -S {name}`")]
    NotInstalled { name: String },
    #[error(
        "runtime `{name}` build {build} is not installed; install it with `sruman -S {name}={build}`"
    )]
    BuildMissing { name: String, build: BuildId },
    #[error("{name} {build} is in use by a running game; stop it first, or force the removal")]
    InUse { name: String, build: BuildId },
    #[error("{name} {build} is already installed")]
    AlreadyInstalled { name: String, build: BuildId },
    #[error("{path}: bad metadata: {message}")]
    Meta { path: PathBuf, message: String },
    #[error(transparent)]
    Mtree(#[from] mtree::MtreeError),
}

fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> StoreError {
    let path = path.into();
    move |source| StoreError::Io { path, source }
}

/// Where an installed build came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// Downloaded and checked against the server's `SHA256SUMS`.
    Download { url: String, sha256: String },
    /// Copied from a directory (`-U <dir>`).
    Import { from: PathBuf },
    /// Registered in place (`-U <dir> --link`); never deleted by sruman.
    Link { target: PathBuf },
}

/// `<build_id>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildMeta {
    /// sruman version that installed it.
    pub sruman: String,
    pub runtime: String,
    pub build: BuildId,
    /// Unix seconds.
    pub installed_at: u64,
    pub source: Source,
}

impl BuildMeta {
    pub fn new(runtime: &str, build: &BuildId, source: Source) -> Self {
        Self {
            sruman: crate::VERSION.to_owned(),
            runtime: runtime.to_owned(),
            build: build.clone(),
            installed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            source,
        }
    }
}

/// One installed build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub build: BuildId,
    /// `<root>/<name>/<build_id>`; a symlink for linked builds.
    pub dir: PathBuf,
    /// `None` when the metadata file is missing or unreadable.
    pub meta: Option<BuildMeta>,
    pub is_current: bool,
}

impl Installed {
    pub fn is_linked(&self) -> bool {
        matches!(
            self.meta,
            Some(BuildMeta {
                source: Source::Link { .. },
                ..
            })
        ) || self.dir.is_symlink()
    }

    /// `name=build`.
    pub fn as_ref(&self) -> RuntimeRef {
        RuntimeRef {
            name: self.name.clone(),
            build: Some(self.build.clone()),
        }
    }
}

/// Proof that this process holds the store lock. Dropping it releases the
/// lock.
#[derive(Debug)]
pub struct StoreLock {
    _file: File,
}

/// A shared hold on one build while a game runs on it. Pass
/// [`BuildHold::fd`] to the child (without `CLOEXEC`) so the hold outlives a
/// detached launcher.
#[derive(Debug)]
pub struct BuildHold {
    file: File,
}

impl BuildHold {
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }

    pub fn into_fd(self) -> OwnedFd {
        self.file.into()
    }
}

/// What [`Store::clean`] keeps.
#[derive(Debug, Clone, Default)]
pub struct CleanPolicy {
    /// Newest builds kept per runtime, besides `current`.
    pub keep: usize,
    /// Builds that must stay (profiles pin them).
    pub protect: Vec<RuntimeRef>,
}

/// Why [`Store::clean`] kept a build it would otherwise remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptBecause {
    InUse,
    Linked,
}

#[derive(Debug, Default)]
pub struct CleanReport {
    pub removed: Vec<Installed>,
    pub kept: Vec<(Installed, KeptBecause)>,
}

/// Whether [`Store::commit`] makes the new build `current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakeCurrent {
    /// When there is no current build, or the new one is newer.
    IfNewer,
    Always,
}

/// The runtime store at one root.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens the store at `root` without touching the filesystem.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Takes the store lock without waiting (pacman's `db.lck`) and wipes
    /// leftovers of interrupted operations.
    pub fn lock(&self) -> Result<StoreLock, StoreError> {
        fs::create_dir_all(&self.root).map_err(io_err(&self.root))?;
        let path = self.root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_err(&path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(StoreError::Busy(path)),
            Err(fs::TryLockError::Error(e)) => return Err(io_err(&path)(e)),
        }
        let staging = self.root.join(STAGING_DIR);
        if staging.exists() {
            remove_tree(&staging)?;
        }
        Ok(StoreLock { _file: file })
    }

    /// A fresh directory under `.staging/` for building a new install. It's
    /// on the store's filesystem, so [`Store::commit`] can rename it into
    /// place.
    pub fn staging_dir(&self, _lock: &StoreLock, label: &str) -> Result<PathBuf, StoreError> {
        let base = self.root.join(STAGING_DIR);
        fs::create_dir_all(&base).map_err(io_err(&base))?;
        for n in 0u32.. {
            let dir = base.join(format!("{label}.{}.{n}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_err(&dir)(e)),
            }
        }
        unreachable!("u32 staging names exhausted")
    }

    fn runtime_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn build_path(&self, name: &str, build: &BuildId) -> PathBuf {
        self.runtime_dir(name).join(build.as_str())
    }

    fn meta_path(&self, name: &str, build: &BuildId) -> PathBuf {
        self.runtime_dir(name).join(format!("{build}.toml"))
    }

    fn hold_path(&self, name: &str, build: &BuildId) -> PathBuf {
        self.runtime_dir(name).join(format!("{build}.lock"))
    }

    /// Every installed build, by name then build.
    pub fn installed(&self) -> Result<Vec<Installed>, StoreError> {
        let mut all = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(all),
            Err(e) => return Err(io_err(&self.root)(e)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_err(&self.root))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && entry.path().is_dir() {
                names.push(name);
            }
        }
        names.sort();
        for name in names {
            all.extend(self.installed_of(&name)?);
        }
        Ok(all)
    }

    /// Installed builds of one runtime, oldest first.
    pub fn installed_of(&self, name: &str) -> Result<Vec<Installed>, StoreError> {
        let dir = self.runtime_dir(name);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(&dir)(e)),
        };
        let current = self.current(name);
        let mut builds = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_err(&dir))?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Ok(build) = file_name.parse::<BuildId>() else {
                continue;
            };
            let path = entry.path();
            // Dangling links (a linked build whose target is gone) still
            // show, so they can be removed.
            if !(path.is_dir() || path.is_symlink()) {
                continue;
            }
            let meta = self.read_meta(name, &build).ok();
            builds.push(Installed {
                is_current: current.as_ref() == Some(&build),
                name: name.to_owned(),
                build,
                dir: path,
                meta,
            });
        }
        builds.sort_by(|a, b| a.build.cmp(&b.build));
        Ok(builds)
    }

    fn read_meta(&self, name: &str, build: &BuildId) -> Result<BuildMeta, StoreError> {
        let path = self.meta_path(name, build);
        let text = fs::read_to_string(&path).map_err(io_err(&path))?;
        toml::from_str(&text).map_err(|e| StoreError::Meta {
            path,
            message: e.to_string(),
        })
    }

    fn write_meta(&self, meta: &BuildMeta) -> Result<(), StoreError> {
        let path = self.meta_path(&meta.runtime, &meta.build);
        let text = toml::to_string(meta).map_err(|e| StoreError::Meta {
            path: path.clone(),
            message: e.to_string(),
        })?;
        write_atomic(&path, text.as_bytes())
    }

    /// The build `current` points at, if it's installed.
    pub fn current(&self, name: &str) -> Option<BuildId> {
        let dir = self.runtime_dir(name);
        let target = fs::read_link(dir.join(CURRENT)).ok()?;
        let build: BuildId = target.to_str()?.parse().ok()?;
        let path = dir.join(build.as_str());
        (path.is_dir() || path.is_symlink()).then_some(build)
    }

    /// `name` resolves to `current` (or the newest build when `current` is
    /// missing); `name=build` to exactly that build.
    pub fn resolve(&self, runtime: &RuntimeRef) -> Result<Installed, StoreError> {
        let builds = self.installed_of(&runtime.name)?;
        match &runtime.build {
            Some(build) => builds
                .into_iter()
                .find(|b| &b.build == build)
                .ok_or_else(|| StoreError::BuildMissing {
                    name: runtime.name.clone(),
                    build: build.clone(),
                }),
            None => {
                let pick = builds
                    .iter()
                    .position(|b| b.is_current)
                    .unwrap_or(builds.len().wrapping_sub(1));
                builds
                    .into_iter()
                    .nth(pick)
                    .ok_or_else(|| StoreError::NotInstalled {
                        name: runtime.name.clone(),
                    })
            }
        }
    }

    /// Points `current` at `build` atomically (new symlink, then rename).
    pub fn set_current(
        &self,
        _lock: &StoreLock,
        name: &str,
        build: &BuildId,
    ) -> Result<(), StoreError> {
        let dir = self.runtime_dir(name);
        let tmp = dir.join(format!(".{CURRENT}.{}", std::process::id()));
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(build.as_str(), &tmp).map_err(io_err(&tmp))?;
        fs::rename(&tmp, dir.join(CURRENT)).map_err(io_err(dir.join(CURRENT)))
    }

    /// Moves a fully prepared build from `staged` (a [`Store::staging_dir`])
    /// into place and records `meta`.
    pub fn commit(
        &self,
        lock: &StoreLock,
        staged: &Path,
        meta: BuildMeta,
        make_current: MakeCurrent,
    ) -> Result<Installed, StoreError> {
        let target = self.prepare_slot(&meta)?;
        fs::rename(staged, &target).map_err(io_err(&target))?;
        self.finish_commit(lock, meta, make_current)
    }

    /// Registers an existing directory in place (`-U --link`).
    pub fn link(
        &self,
        lock: &StoreLock,
        target_dir: &Path,
        meta: BuildMeta,
        make_current: MakeCurrent,
    ) -> Result<Installed, StoreError> {
        let slot = self.prepare_slot(&meta)?;
        std::os::unix::fs::symlink(target_dir, &slot).map_err(io_err(&slot))?;
        self.finish_commit(lock, meta, make_current)
    }

    fn prepare_slot(&self, meta: &BuildMeta) -> Result<PathBuf, StoreError> {
        let dir = self.runtime_dir(&meta.runtime);
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let slot = self.build_path(&meta.runtime, &meta.build);
        if slot.exists() || slot.is_symlink() {
            return Err(StoreError::AlreadyInstalled {
                name: meta.runtime.clone(),
                build: meta.build.clone(),
            });
        }
        Ok(slot)
    }

    fn finish_commit(
        &self,
        lock: &StoreLock,
        meta: BuildMeta,
        make_current: MakeCurrent,
    ) -> Result<Installed, StoreError> {
        self.write_meta(&meta)?;
        let promote = match (make_current, self.current(&meta.runtime)) {
            (MakeCurrent::Always, _) | (MakeCurrent::IfNewer, None) => true,
            (MakeCurrent::IfNewer, Some(current)) => meta.build > current,
        };
        if promote {
            self.set_current(lock, &meta.runtime, &meta.build)?;
        }
        self.resolve(&RuntimeRef {
            name: meta.runtime.clone(),
            build: Some(meta.build),
        })
    }

    /// Whether a game runs on `build`: sruman's own hold on
    /// `<build_id>.lock`, or pressure-vessel's lock on a runtime copy in
    /// `var/tmp-*/.ref` (any launcher: Steam, umu, lootrs, sruman).
    pub fn in_use(&self, build: &Installed) -> bool {
        held_by_sruman(&self.hold_path(&build.name, &build.build))
            || held_by_pressure_vessel(&build.dir.join("var"))
    }

    /// The package list shipped inside a container build
    /// (`<suite>_platform_<build>/files/manifest.dpkg`); `None` for builds
    /// without one (scout-ldlp) or when it's unreadable.
    pub fn manifest(&self, build: &Installed) -> Option<crate::manifest::Manifest> {
        let entries = fs::read_dir(&build.dir).ok()?;
        let path = entries
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("_platform_"))
                    && path.join("files/manifest.dpkg").is_file()
            })?
            .join("files/manifest.dpkg");
        crate::manifest::Manifest::parse(&fs::read_to_string(path).ok()?).ok()
    }

    /// Takes a shared hold on `build` for a game launch.
    pub fn hold(&self, build: &Installed) -> Result<BuildHold, StoreError> {
        let path = self.hold_path(&build.name, &build.build);
        let file = open_lock_file(&path)?;
        match file.try_lock_shared() {
            Ok(()) => Ok(BuildHold { file }),
            // Only a removal holds it exclusively.
            Err(fs::TryLockError::WouldBlock) => Err(StoreError::Busy(path)),
            Err(fs::TryLockError::Error(e)) => Err(io_err(&path)(e)),
        }
    }

    /// Removes one build. A linked build is only unregistered. Fails with
    /// [`StoreError::InUse`] unless `force`.
    pub fn remove(
        &self,
        lock: &StoreLock,
        build: &Installed,
        force: bool,
    ) -> Result<(), StoreError> {
        let hold_path = self.hold_path(&build.name, &build.build);
        // Exclusive on the hold file keeps new launches out while deleting.
        let hold = open_lock_file(&hold_path)?;
        let ours_free = hold.try_lock().is_ok();
        if !force && (!ours_free || held_by_pressure_vessel(&build.dir.join("var"))) {
            return Err(StoreError::InUse {
                name: build.name.clone(),
                build: build.build.clone(),
            });
        }
        if build.dir.is_symlink() {
            fs::remove_file(&build.dir).map_err(io_err(&build.dir))?;
        } else if build.dir.exists() {
            // Rename first so the build disappears at once, then delete.
            let trash = self.staging_dir(lock, &format!("trash-{}-{}", build.name, build.build))?;
            let moved = trash.join("build");
            fs::rename(&build.dir, &moved).map_err(io_err(&build.dir))?;
            remove_tree(&trash)?;
        }
        for path in [self.meta_path(&build.name, &build.build), hold_path] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(&path)(e)),
            }
        }
        drop(hold);

        let left = self.installed_of(&build.name)?;
        let current_link = self.runtime_dir(&build.name).join(CURRENT);
        match left.last() {
            None => {
                let _ = fs::remove_file(&current_link);
                let _ = fs::remove_dir(self.runtime_dir(&build.name));
            }
            Some(newest) if build.is_current || self.current(&build.name).is_none() => {
                self.set_current(lock, &build.name, &newest.build)?;
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Removes old builds: per runtime, keeps `current`, the `keep` newest,
    /// protected (pinned) builds, linked builds and builds in use.
    pub fn clean(&self, lock: &StoreLock, policy: &CleanPolicy) -> Result<CleanReport, StoreError> {
        let mut report = CleanReport::default();
        let all = self.installed()?;
        let mut names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        names.dedup();
        for name in names {
            let mut builds: Vec<&Installed> = all.iter().filter(|b| b.name == name).collect();
            builds.reverse(); // newest first
            for (rank, build) in builds.into_iter().enumerate() {
                let protected = policy
                    .protect
                    .iter()
                    .any(|r| r.name == build.name && r.build.as_ref() == Some(&build.build));
                if build.is_current || rank < policy.keep || protected {
                    continue;
                }
                if build.is_linked() {
                    report.kept.push((build.clone(), KeptBecause::Linked));
                    continue;
                }
                match self.remove(lock, build, false) {
                    Ok(()) => report.removed.push(build.clone()),
                    Err(StoreError::InUse { .. }) => {
                        report.kept.push((build.clone(), KeptBecause::InUse));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(report)
    }

    /// Checks a build against its own file lists (`-Qk`/`-Qkk`): the
    /// top-level `mtree.txt.gz`, and each platform's `usr-mtree.txt.gz`.
    /// `None` when the build ships no file list (scout-ldlp).
    pub fn verify(
        &self,
        build: &Installed,
        depth: Depth,
    ) -> Result<Option<mtree::Report>, StoreError> {
        let top = build.dir.join("mtree.txt.gz");
        if !top.is_file() {
            return Ok(None);
        }
        let mut report = mtree::verify(&build.dir, &mtree::read_gz(&top)?, depth, false);
        let entries = fs::read_dir(&build.dir).map_err(io_err(&build.dir))?;
        for entry in entries {
            let platform = entry.map_err(io_err(&build.dir))?.path();
            let usr = platform.join("usr-mtree.txt.gz");
            if usr.is_file() {
                let sub =
                    mtree::verify(&platform.join("files"), &mtree::read_gz(&usr)?, depth, true);
                let prefix = platform
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join("files");
                report.checked += sub.checked;
                report
                    .problems
                    .extend(sub.problems.into_iter().map(|p| prefix_problem(&prefix, p)));
            }
        }
        Ok(Some(report))
    }
}

fn prefix_problem(prefix: &Path, problem: mtree::Problem) -> mtree::Problem {
    use mtree::Problem as P;
    match problem {
        P::Missing(p) => P::Missing(prefix.join(p)),
        P::WrongType(p) => P::WrongType(prefix.join(p)),
        P::WrongHash(p) => P::WrongHash(prefix.join(p)),
        P::WrongSize {
            path,
            expected,
            found,
        } => P::WrongSize {
            path: prefix.join(path),
            expected,
            found,
        },
        P::WrongLink {
            path,
            expected,
            found,
        } => P::WrongLink {
            path: prefix.join(path),
            expected,
            found,
        },
        P::Unreadable { path, message } => P::Unreadable {
            path: prefix.join(path),
            message,
        },
    }
}

fn open_lock_file(path: &Path) -> Result<File, StoreError> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io_err(path))
}

/// Someone holds the build's sruman lock file.
fn held_by_sruman(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().write(true).open(path) else {
        return false; // never held: launches create it
    };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
}

/// pressure-vessel holds a read OFD lock on `<var>/tmp-*/.ref` while a
/// container runs. A POSIX write-lock probe conflicts with it. An
/// unreadable `.ref` counts as in use, to stay on the safe side.
fn held_by_pressure_vessel(var: &Path) -> bool {
    let Ok(entries) = fs::read_dir(var) else {
        return false;
    };
    entries.flatten().any(|entry| {
        if !entry.file_name().to_string_lossy().starts_with("tmp-") {
            return false;
        }
        let ref_file = entry.path().join(".ref");
        match OpenOptions::new().read(true).write(true).open(&ref_file) {
            Ok(file) => {
                rustix::fs::fcntl_lock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                    .is_err()
            }
            Err(e) => e.kind() != io::ErrorKind::NotFound,
        }
    })
}

/// Writes through a temporary file and a rename.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, data).map_err(io_err(&tmp))?;
    fs::rename(&tmp, path).map_err(io_err(path))
}

/// `remove_dir_all`, making read-only directories writable when needed.
pub(crate) fn remove_tree(path: &Path) -> Result<(), StoreError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            make_dirs_writable(path);
            fs::remove_dir_all(path).map_err(io_err(path))
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

pub(crate) fn make_dirs_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mut perms = meta.permissions();
    perms.set_mode(perms.mode() | 0o700);
    let _ = fs::set_permissions(path, perms);
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            make_dirs_writable(&entry.path());
        }
    }
}
