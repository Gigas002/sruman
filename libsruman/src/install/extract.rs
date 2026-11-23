//! Safe extraction of Valve's `.tar.xz` archives.
//!
//! Valve's archives hold only regular files, directories and relative
//! symlinks that stay inside the archive, under one top-level directory
//! (verified in the spike). So everything else is refused outright instead
//! of being handled: absolute or `..` paths, hardlinks, devices, FIFOs,
//! absolute or escaping symlinks, and any entry that would be written
//! through a symlink. The `tar` crate only parses; every file, directory
//! and link is created here, in a fresh staging directory, with
//! `create_new` semantics. Modes keep the permission bits only (no setuid,
//! setgid or sticky), and the owner can always write, since pressure-vessel
//! needs a writable build and `remove` must be able to delete it.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek},
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use tar::EntryType;

use crate::progress::{CancelFlag, Cancelled, Progress};

/// Errors from unpacking a runtime archive.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// The archive unpacks to more than [`Limits`] allow.
    #[error("the archive unpacks to more than {limit}; it can't be a runtime build")]
    TooLarge {
        /// The limit that was hit, for the message: `4096 MiB` or `1000000 entries`.
        limit: String,
    },
    /// An entry sruman won't unpack (a path escaping the root, a device node, ...).
    #[error("archive entry `{path}`: {reason}")]
    Refused {
        /// The entry's path in the archive.
        path: String,
        /// Why it was refused.
        reason: String,
    },
    /// Entries under two different top-level directories (the first two seen).
    #[error("the archive has no single top-level directory (`{0}` and `{1}`)")]
    TopLevel(String, String),
    /// The archive has no entries.
    #[error("the archive is empty")]
    Empty,
    /// A filesystem operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What sruman was doing.
        context: String,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The operation was cancelled.
    #[error(transparent)]
    Cancelled(#[from] Cancelled),
}

fn io_err(context: impl Into<String>) -> impl FnOnce(io::Error) -> ExtractError {
    let context = context.into();
    move |source| {
        if crate::progress::is_cancelled(&source) {
            ExtractError::Cancelled(Cancelled)
        } else {
            ExtractError::Io { context, source }
        }
    }
}

/// A file that tracks how far it has been read contiguously from the
/// start, for progress. The xz decoder first reads the index at the end of
/// the file, then the blocks in order; only the latter count.
struct Counting<R> {
    inner: R,
    pos: u64,
    done: Arc<AtomicU64>,
    cancel: CancelFlag,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other(Cancelled));
        }
        let n = self.inner.read(buf)?;
        let start = self.pos;
        self.pos += n as u64;
        if start <= self.done.load(Ordering::Relaxed) {
            self.done.fetch_max(self.pos, Ordering::Relaxed);
        }
        Ok(n)
    }
}

impl<R: Seek> Seek for Counting<R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        self.pos = self.inner.seek(pos)?;
        Ok(self.pos)
    }
}

/// Decoder threads. Each holds a whole xz block, compressed and unpacked
/// (Valve's blocks unpack to 24 MiB), so memory grows about 70 MiB per
/// worker; past two, the single tar consumer is the bottleneck and more
/// workers only cost memory (docs/AUDIT.md: 2 workers 0.85 s / 183 MiB on
/// an 800 MiB build, 12 workers 1.06 s / 647 MiB, streaming 2.9 s / 19 MiB).
fn workers() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(2) as u32)
}

/// Bounds on what an archive may unpack to, so a hostile or broken archive
/// can't fill the disk or its inodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Total bytes of regular files.
    pub bytes: u64,
    /// Number of entries.
    pub entries: u64,
}

impl Limits {
    /// At least this many bytes are always allowed.
    pub const MIN_BYTES: u64 = 512 << 20;
    /// Entries allowed (Valve's builds have a few tens of thousands).
    pub const ENTRIES: u64 = 1_000_000;

    /// Twice the packages' `Installed-Size` when the manifest is known
    /// (unpacked builds come out smaller: documentation is stripped), else
    /// 20 times the compressed archive (Valve's archives unpack to 3-5
    /// times their size); never below [`Limits::MIN_BYTES`].
    pub fn for_build(installed_size_kib: Option<u64>, archive_bytes: u64) -> Self {
        let bytes = installed_size_kib.map_or_else(
            || archive_bytes.saturating_mul(20),
            |kib| kib.saturating_mul(1024).saturating_mul(2),
        );
        Self {
            bytes: bytes.max(Self::MIN_BYTES),
            entries: Self::ENTRIES,
        }
    }
}

/// Extracts the `.tar.xz` at `archive` into the empty directory `dest`,
/// stripping the single top-level directory. Progress counts compressed
/// bytes (`total` = the archive's size). Unpacking stops with
/// [`ExtractError::TooLarge`] past `limits`. Returns the top-level name.
pub fn extract_tar_xz(
    archive: &Path,
    dest: &Path,
    limits: Limits,
    progress: &mut dyn Progress,
    cancel: &CancelFlag,
) -> Result<String, ExtractError> {
    let file = File::open(archive).map_err(io_err(format!("opening {}", archive.display())))?;
    let read = Arc::new(AtomicU64::new(0));
    let counting = Counting {
        inner: file,
        pos: 0,
        done: Arc::clone(&read),
        cancel: cancel.clone(),
    };
    let xz = lzma_rust2::XzReaderMt::new(counting, false, workers())
        .map_err(io_err(format!("reading {}", archive.display())))?;
    let mut reported = 0;
    let mut tick = |progress: &mut dyn Progress| {
        let now = read.load(Ordering::Relaxed);
        if now > reported {
            progress.advance(now - reported);
            reported = now;
        }
    };
    let top = extract_tar(xz, dest, limits, cancel, &mut || tick(progress)).map_err(|err| {
        // The decoder threads may surface a cancelled read as some other
        // error (a short stream, a wrapped error): the flag decides.
        if cancel.is_cancelled() {
            ExtractError::Cancelled(Cancelled)
        } else {
            err
        }
    })?;
    // The index and footer at the end were read first and never counted.
    let size = fs::metadata(archive).map_or(0, |m| m.len());
    read.fetch_max(size, Ordering::Relaxed);
    tick(progress);
    Ok(top)
}

/// Extracts a tar stream (see [`extract_tar_xz`]); `on_entry` runs after
/// each entry.
pub fn extract_tar(
    reader: impl Read,
    dest: &Path,
    limits: Limits,
    cancel: &CancelFlag,
    on_entry: &mut dyn FnMut(),
) -> Result<String, ExtractError> {
    let mut archive = tar::Archive::new(reader);
    let mut top: Option<String> = None;
    let mut links: HashSet<PathBuf> = HashSet::new();
    let mut dir_modes: Vec<(PathBuf, u32)> = Vec::new();
    let entries = archive.entries().map_err(io_err("reading the archive"))?;
    let (mut bytes, mut count) = (0u64, 0u64);
    for entry in entries {
        cancel.check()?;
        let mut entry = entry.map_err(io_err("reading the archive"))?;
        count += 1;
        if count > limits.entries {
            return Err(ExtractError::TooLarge {
                limit: format!("{} entries", limits.entries),
            });
        }
        // The entry's data is exactly this long (PAX size included), so
        // checking before writing bounds what lands on disk.
        bytes = bytes.saturating_add(entry.size());
        if bytes > limits.bytes {
            return Err(ExtractError::TooLarge {
                limit: format!("{} MiB", limits.bytes >> 20),
            });
        }
        let raw = entry.path_bytes().into_owned();
        let shown = String::from_utf8_lossy(&raw).into_owned();
        let refuse = |reason: &str| ExtractError::Refused {
            path: shown.clone(),
            reason: reason.to_owned(),
        };
        let path = entry
            .path()
            .map_err(io_err(format!("archive entry `{shown}`")))?;
        let (first, rel) = split_top(&path).ok_or_else(|| refuse("not a plain relative path"))?;
        match &top {
            None => top = Some(first.clone()),
            Some(top) if *top != first => return Err(ExtractError::TopLevel(top.clone(), first)),
            Some(_) => {}
        }
        let kind = entry.header().entry_type();
        let mode = entry
            .header()
            .mode()
            .map_err(io_err(format!("archive entry `{shown}`")))?;
        if rel.as_os_str().is_empty() {
            // The top-level directory itself.
            if kind != EntryType::Directory {
                return Err(refuse("the top level must be a directory"));
            }
            continue;
        }
        if rel.ancestors().skip(1).any(|a| links.contains(a)) || links.contains(&rel) {
            return Err(refuse("would be written through or over a symlink"));
        }
        let target = dest.join(&rel);
        let context = || format!("extracting {}", target.display());
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_err(context()))?;
        }
        match kind {
            EntryType::Directory => {
                match fs::create_dir(&target) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists && target.is_dir() => {}
                    other => other.map_err(io_err(context()))?,
                }
                dir_modes.push((target, (mode & 0o777) | 0o700));
            }
            EntryType::Regular | EntryType::Continuous => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode((mode & 0o777) | 0o600)
                    .open(&target)
                    .map_err(io_err(context()))?;
                io::copy(&mut entry, &mut file).map_err(io_err(context()))?;
                // The umask may have dropped bits; set them exactly.
                file.set_permissions(fs::Permissions::from_mode((mode & 0o777) | 0o600))
                    .map_err(io_err(context()))?;
                if let Ok(mtime) = entry.header().mtime() {
                    let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime));
                }
            }
            EntryType::Symlink => {
                let link = entry
                    .link_name()
                    .map_err(io_err(context()))?
                    .ok_or_else(|| refuse("symlink without a target"))?;
                if !symlink_stays_inside(&rel, &link) {
                    return Err(refuse(&format!(
                        "symlink to `{}` leaves the archive",
                        link.display()
                    )));
                }
                symlink(&link, &target).map_err(io_err(context()))?;
                links.insert(rel);
            }
            EntryType::Link => return Err(refuse("hardlinks are not supported")),
            other => return Err(refuse(&format!("unsupported entry type {other:?}"))),
        }
        on_entry();
    }
    // Deepest first, so a read-only parent never blocks its children.
    dir_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (dir, mode) in dir_modes {
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode))
            .map_err(io_err(format!("setting the mode of {}", dir.display())))?;
    }
    top.ok_or(ExtractError::Empty)
}

/// `top/rest` → `(top, rest)`, allowing a leading `./`; `None` for
/// absolute paths, `..` or anything else that isn't plain.
pub(super) fn split_top(path: &Path) -> Option<(String, PathBuf)> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    let (first, rest) = parts.split_first()?;
    Some((first.to_str()?.to_owned(), rest.iter().collect()))
}

/// Whether a symlink at `rel` (relative to the root) pointing to `link`
/// stays inside the root: relative, with `..` only as a leading prefix that
/// doesn't climb above the root.
///
/// A purely lexical check isn't enough: with `s -> .`, the target `s/..`
/// looks inside but resolves to the root's parent. With `..` only up
/// front, resolution climbs through real directories (entries are never
/// written through symlinks), then only descends, so every link stays
/// inside by induction. All 1,520 symlinks in Valve's archives have this
/// form.
pub(super) fn symlink_stays_inside(rel: &Path, link: &Path) -> bool {
    if link.as_os_str().is_empty() {
        return false;
    }
    // Depth of the directory holding the link.
    let mut depth = rel.components().count() as i64 - 1;
    let mut descended = false;
    for component in link.components() {
        match component {
            Component::Normal(_) => descended = true,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if descended || depth < 0 {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}
