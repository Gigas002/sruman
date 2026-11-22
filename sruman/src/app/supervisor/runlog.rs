//! Per-run logs, in lootrs' layout and format:
//!
//! ```text
//! <logs>/games/<game>/<stamp>.log     # one per run
//! <logs>/games/<game>/latest.log      # link to the newest
//! <logs>/exec/<profile>/…             # `execute`
//! <logs>/init/<profile>/…             # `init`
//! ```
//!
//! Logs are `0600`: they hold the environment sruman set.

use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const LATEST: &str = "latest.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind<'a> {
    Game(&'a str),
    Execute(&'a str),
    Init(&'a str),
}

impl RunKind<'_> {
    /// The directory of this game's or profile's logs.
    pub fn dir(&self, logs: &Path) -> PathBuf {
        let (group, name) = match self {
            Self::Game(name) => ("games", name),
            Self::Execute(name) => ("exec", name),
            Self::Init(name) => ("init", name),
        };
        logs.join(group).join(name)
    }
}

/// Creates `<stamp>.log` (`<stamp>-2.log`, … within one second), keeps at
/// most `keep` logs counting the new one, and points `latest.log` at it.
pub fn create(logs: &Path, kind: RunKind, stamp: &str, keep: usize) -> io::Result<(File, PathBuf)> {
    let dir = kind.dir(logs);
    std::fs::create_dir_all(&dir)?;
    prune(&dir, keep.saturating_sub(1))?;
    // After the newest log of this second, never in a gap left by pruning.
    let first = list(&dir)?
        .iter()
        .map(|log| age(log))
        .filter(|(log_stamp, _)| *log_stamp == stamp)
        .map(|(_, n)| n.saturating_add(1))
        .max()
        .unwrap_or(1);
    for n in first.. {
        let stem = match n {
            1 => stamp.to_owned(),
            n => format!("{stamp}-{n}"),
        };
        let path = dir.join(format!("{stem}.log"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => {
                link_latest(&dir, &path)?;
                return Ok((file, path));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    unreachable!("an unbounded range")
}

/// The newest log of `kind`, through `latest.log`.
pub fn latest(logs: &Path, kind: RunKind) -> Option<PathBuf> {
    let link = kind.dir(logs).join(LATEST);
    let target = std::fs::read_link(&link).ok()?;
    let path = link.parent()?.join(target);
    path.is_file().then_some(path)
}

/// Deletes the oldest logs until at most `keep` remain.
fn prune(dir: &Path, keep: usize) -> io::Result<()> {
    let logs = list(dir)?;
    for log in &logs[..logs.len().saturating_sub(keep)] {
        std::fs::remove_file(log)?;
    }
    Ok(())
}

/// The run logs in `dir`, oldest first.
fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "log")
                && path.file_name().is_some_and(|name| name != LATEST)
                && !path.is_symlink()
        })
        .collect();
    logs.sort_by(|a, b| age(a).cmp(&age(b)));
    Ok(logs)
}

/// Sort key, oldest first: the stamp (`YYYYMMDD-HHMMSS` sorts as text),
/// then the `-2`, `-3`, … of runs in the same second.
fn age(log: &Path) -> (&str, u32) {
    let stem = log
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    match stem.rsplit_once('-') {
        Some((stamp, n)) if stamp.contains('-') => match n.parse() {
            Ok(n) => (stamp, n),
            Err(_) => (stem, 1),
        },
        _ => (stem, 1),
    }
}

/// Repoints `latest.log` (a relative link) at `target` atomically.
fn link_latest(dir: &Path, target: &Path) -> io::Result<()> {
    // Unique per process and call: two runs starting together must not
    // remove each other's temporary link.
    static LINKS: AtomicU64 = AtomicU64::new(0);
    let name = target.file_name().unwrap_or_default();
    let tmp = dir.join(format!(
        ".latest.log.{}.{}.tmp",
        std::process::id(),
        LINKS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(name, &tmp)?;
    std::fs::rename(&tmp, dir.join(LATEST)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rotation_and_latest() {
        let logs = tempfile::tempdir().unwrap();
        let kind = RunKind::Game("hk");
        assert_eq!(latest(logs.path(), kind), None);
        let (_, first) = create(logs.path(), kind, "20261122-100000", 2).unwrap();
        assert_eq!(first, logs.path().join("games/hk/20261122-100000.log"));
        let (_, second) = create(logs.path(), kind, "20261122-100000", 2).unwrap();
        assert_eq!(second, logs.path().join("games/hk/20261122-100000-2.log"));
        assert_eq!(latest(logs.path(), kind), Some(second.clone()));
        // keep = 2: the first one goes.
        let (_, third) = create(logs.path(), kind, "20261122-100001", 2).unwrap();
        assert!(!first.exists() && second.exists() && third.exists());
        assert_eq!(latest(logs.path(), kind), Some(third));

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&second).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let (_, exec) = create(logs.path(), RunKind::Execute("sniper"), "x-1", 1).unwrap();
        assert!(exec.starts_with(logs.path().join("exec/sniper")));
        let (_, init) = create(logs.path(), RunKind::Init("sniper"), "x-1", 1).unwrap();
        assert!(init.starts_with(logs.path().join("init/sniper")));
    }

    #[test]
    fn age_orders_same_second_runs() {
        let mut logs = [
            PathBuf::from("20261122-100000-10.log"),
            PathBuf::from("20261122-100000-2.log"),
            PathBuf::from("20261122-100000.log"),
            PathBuf::from("20261121-235959.log"),
        ];
        logs.sort_by(|a, b| age(a).cmp(&age(b)));
        let names: Vec<_> = logs.iter().map(|p| p.to_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "20261121-235959.log",
                "20261122-100000.log",
                "20261122-100000-2.log",
                "20261122-100000-10.log"
            ]
        );
    }
}
