//! Downloads into the package cache, resuming partial files and checking
//! SHA-256 while streaming.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::InstallError;
use crate::{
    fetch::Fetch,
    progress::{CancelFlag, Phase, Progress, ProgressReader, is_cancelled},
};

/// What a download produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    /// The cache already had a verified copy.
    Cached,
    Downloaded,
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    name.into()
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn io_err(context: String) -> impl FnOnce(io::Error) -> InstallError {
    move |source| {
        if is_cancelled(&source) {
            InstallError::Cancelled
        } else {
            InstallError::Io { context, source }
        }
    }
}

/// Hashes `path` into `hasher`, reporting through `progress`.
fn hash_into(
    path: &Path,
    hasher: &mut Sha256,
    progress: &mut dyn Progress,
    cancel: &CancelFlag,
) -> Result<(), InstallError> {
    let file = File::open(path).map_err(io_err(format!("reading {}", path.display())))?;
    let mut reader = ProgressReader::new(file, progress, cancel);
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(io_err(format!("reading {}", path.display())))?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

/// Makes `dest` a verified copy of `url` (SHA-256 `sha256`): reuses a
/// cached copy that checks out, resumes `dest.part` with `Range`, and
/// starts over once if a resumed file fails the check. A failed or
/// cancelled download leaves `dest.part` for the next attempt.
pub fn download(
    fetch: &dyn Fetch,
    url: &str,
    sha256: &str,
    size: Option<u64>,
    dest: &Path,
    progress: &mut dyn Progress,
    cancel: &CancelFlag,
) -> Result<Fetched, InstallError> {
    if dest.is_file() {
        progress.phase(Phase::Verify, size);
        let mut hasher = Sha256::new();
        hash_into(dest, &mut hasher, progress, cancel)?;
        if hex(&hasher.finalize()) == sha256 {
            return Ok(Fetched::Cached);
        }
        fs::remove_file(dest).map_err(io_err(format!("removing {}", dest.display())))?;
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(io_err(format!("creating {}", dir.display())))?;
    }
    let part = part_path(dest);
    for attempt in 0..2 {
        let resumed = attempt == 0 && part.is_file();
        let actual = download_part(fetch, url, size, &part, resumed, progress, cancel)?;
        if actual == sha256 {
            fs::rename(&part, dest).map_err(io_err(format!("moving {}", dest.display())))?;
            return Ok(Fetched::Downloaded);
        }
        let _ = fs::remove_file(&part);
        if !resumed {
            return Err(InstallError::Checksum {
                url: url.to_owned(),
                expected: sha256.to_owned(),
                actual,
            });
        }
        // A resumed file can be stale (the server's file changed between
        // attempts): try once more from scratch.
    }
    unreachable!("the second attempt never resumes")
}

/// Downloads into `part` (continuing it when `resume`) and returns the
/// whole file's SHA-256.
fn download_part(
    fetch: &dyn Fetch,
    url: &str,
    size: Option<u64>,
    part: &Path,
    resume: bool,
    progress: &mut dyn Progress,
    cancel: &CancelFlag,
) -> Result<String, InstallError> {
    let mut hasher = Sha256::new();
    let mut offset = if resume {
        fs::metadata(part).map_or(0, |m| m.len())
    } else {
        0
    };
    if size.is_some_and(|size| offset > size) {
        offset = 0;
    }
    if offset > 0 {
        // Hash what's there, shown as the start of the download.
        progress.phase(Phase::Download, size);
        hash_into(part, &mut hasher, progress, cancel)?;
    }
    let body = fetch.get_from(url, offset)?;
    let mut file = if body.start == 0 {
        // A fresh download, or the server ignored the Range.
        if offset > 0 {
            hasher = Sha256::new();
        }
        progress.phase(Phase::Download, body.total.or(size));
        File::create(part)
    } else {
        OpenOptions::new().append(true).open(part)
    }
    .map_err(io_err(format!("writing {}", part.display())))?;

    let mut reader = ProgressReader::new(body.reader, progress, cancel);
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = reader.read(&mut buf).map_err(|source| {
            if is_cancelled(&source) {
                InstallError::Cancelled
            } else {
                InstallError::Fetch(crate::fetch::FetchError::Io {
                    url: url.to_owned(),
                    source,
                })
            }
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(io_err(format!("writing {}", part.display())))?;
    }
    file.sync_all()
        .map_err(io_err(format!("writing {}", part.display())))?;
    Ok(hex(&hasher.finalize()))
}
