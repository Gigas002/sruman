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
//! needs a writable build and `-R` must be able to delete it.

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

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("archive entry `{path}`: {reason}")]
    Refused { path: String, reason: String },
    #[error("the archive has no single top-level directory (`{0}` and `{1}`)")]
    TopLevel(String, String),
    #[error("the archive is empty")]
    Empty,
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
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

/// Decoder threads: enough to keep up with the disk, bounded so a big
/// machine doesn't spend memory on idle workers.
fn workers() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(16) as u32)
}

/// Extracts the `.tar.xz` at `archive` into the empty directory `dest`,
/// stripping the single top-level directory. Progress counts compressed
/// bytes (`total` = the archive's size). Returns the top-level name.
pub fn extract_tar_xz(
    archive: &Path,
    dest: &Path,
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
    let top = extract_tar(xz, dest, cancel, &mut || tick(progress))?;
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
    cancel: &CancelFlag,
    on_entry: &mut dyn FnMut(),
) -> Result<String, ExtractError> {
    let mut archive = tar::Archive::new(reader);
    let mut top: Option<String> = None;
    let mut links: HashSet<PathBuf> = HashSet::new();
    let mut dir_modes: Vec<(PathBuf, u32)> = Vec::new();
    let entries = archive.entries().map_err(io_err("reading the archive"))?;
    for entry in entries {
        cancel.check()?;
        let mut entry = entry.map_err(io_err("reading the archive"))?;
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
