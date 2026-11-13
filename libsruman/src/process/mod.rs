//! Running sessions (`-Xs`) and stopping them (`-Xk`), found by the tags
//! [`crate::launch`] sets: `SRUMAN_GAME=<name>` or
//! `SRUMAN_SESSION=<kind>:<profile>:<id>`.
//!
//! Every process of a launch inherits the tag, so `/proc/<pid>/environ`
//! finds them all, whichever tool started them and wherever they are in
//! the tree. sruman starts each launch in its own session (`setsid`), so
//! the session's process group is stopped as a whole: `SIGTERM`, a grace
//! period, then `SIGKILL` for whatever is left, tagged stragglers that left
//! the group included. (In the spike, `SIGTERM` to pressure-vessel's `run`
//! took the whole container down.) Nothing here spawns processes.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rustix::process::{Pid, Signal, kill_process, kill_process_group};

use crate::launch::{TAG_GAME, TAG_PREFIX, TAG_PROFILE, TAG_SESSION};

#[cfg(test)]
mod tests;

/// What a session runs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionTag {
    /// `SRUMAN_GAME`: a game from `games.toml`.
    Game(String),
    /// `SRUMAN_SESSION`: a path run or a shell.
    Session(String),
}

impl std::fmt::Display for SessionTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Game(name) => write!(f, "{name}"),
            Self::Session(id) => write!(f, "{id}"),
        }
    }
}

/// One process, as `/proc` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    /// Clock ticks after boot.
    pub start: u64,
    pub cmdline: Vec<String>,
}

/// The processes sharing one tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub tag: SessionTag,
    pub profile: Option<String>,
    pub prefix: Option<PathBuf>,
    /// The oldest process whose parent isn't in the session: the launcher's
    /// child (wrapper or runtime entry point).
    pub leader: ProcessInfo,
    /// Every process, oldest first.
    pub processes: Vec<ProcessInfo>,
}

/// Reads `KEY=value` pairs from an `environ` file.
fn parse_environ(data: &[u8]) -> BTreeMap<String, String> {
    data.split(|b| *b == 0)
        .filter_map(|entry| {
            let entry = std::str::from_utf8(entry).ok()?;
            let (key, value) = entry.split_once('=')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Parses `/proc/<pid>/stat`: fields after the `(comm)`, which may itself
/// contain spaces and parentheses.
fn parse_stat(text: &str) -> Option<(i32, i32, u64)> {
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt
    // utime stime cutime cstime priority nice threads itrealvalue starttime
    let ppid = fields.get(1)?.parse().ok()?;
    let pgid = fields.get(2)?.parse().ok()?;
    let start = fields.get(19)?.parse().ok()?;
    Some((ppid, pgid, start))
}

fn read_process(dir: &Path, pid: i32) -> Option<(ProcessInfo, BTreeMap<String, String>)> {
    let env = parse_environ(&fs::read(dir.join("environ")).ok()?);
    let (ppid, pgid, start) = parse_stat(&fs::read_to_string(dir.join("stat")).ok()?)?;
    let cmdline = fs::read(dir.join("cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    Some((
        ProcessInfo {
            pid,
            ppid,
            pgid,
            start,
            cmdline,
        },
        env,
    ))
}

/// A session while `/proc` is still being read.
#[derive(Default)]
struct Partial {
    processes: Vec<ProcessInfo>,
    profile: Option<String>,
    prefix: Option<PathBuf>,
}

/// Every tagged session under `proc_root` (`/proc`), sorted by tag.
/// Processes that can't be read (other users', gone meanwhile) are skipped.
pub fn list_in(proc_root: &Path) -> Vec<Session> {
    let mut found: BTreeMap<SessionTag, Partial> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(proc_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let Some((info, env)) = read_process(&entry.path(), pid) else {
            continue;
        };
        let tag = match (env.get(TAG_GAME), env.get(TAG_SESSION)) {
            (Some(game), _) => SessionTag::Game(game.clone()),
            (None, Some(session)) => SessionTag::Session(session.clone()),
            (None, None) => continue,
        };
        let slot = found.entry(tag).or_default();
        slot.processes.push(info);
        if slot.profile.is_none() {
            slot.profile = env.get(TAG_PROFILE).cloned();
        }
        if slot.prefix.is_none() {
            slot.prefix = env.get(TAG_PREFIX).map(PathBuf::from);
        }
    }
    found
        .into_iter()
        .filter_map(|(tag, partial)| {
            let Partial {
                mut processes,
                profile,
                prefix,
            } = partial;
            processes.sort_by_key(|p| (p.start, p.pid));
            let pids: Vec<i32> = processes.iter().map(|p| p.pid).collect();
            let leader = processes
                .iter()
                .find(|p| !pids.contains(&p.ppid))
                .or(processes.first())?
                .clone();
            Some(Session {
                tag,
                profile,
                prefix,
                leader,
                processes,
            })
        })
        .collect()
}

/// [`list_in`] on `/proc`.
pub fn list() -> Vec<Session> {
    list_in(Path::new("/proc"))
}

/// The running session for game `name`, if any (one instance per game).
pub fn find_game(name: &str) -> Option<Session> {
    let tag = SessionTag::Game(name.to_owned());
    list().into_iter().find(|s| s.tag == tag)
}

/// How [`stop`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Everything exited after `SIGTERM`.
    Terminated,
    /// Something was still there after the grace period and got `SIGKILL`.
    Killed,
}

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error("signalling process {pid}: {source}")]
    Signal {
        pid: i32,
        #[source]
        source: std::io::Error,
    },
    #[error("session `{0}` is still running after SIGKILL")]
    Stuck(SessionTag),
}

fn signal_session(
    session: &Session,
    alive: &[ProcessInfo],
    signal: Signal,
) -> Result<(), StopError> {
    // The whole group first, then any tagged process outside it.
    let mut groups: Vec<i32> = alive.iter().map(|p| p.pgid).collect();
    groups.push(session.leader.pgid);
    groups.sort_unstable();
    groups.dedup();
    // Never our own group (a launch that wasn't given its own session).
    let own = rustix::process::getpgrp().as_raw_pid();
    for pgid in groups.into_iter().filter(|g| *g > 1 && *g != own) {
        if let Some(pid) = Pid::from_raw(pgid) {
            // Only groups the session leads: never sruman's own group.
            if pgid == session.leader.pgid || alive.iter().any(|p| p.pid == pgid) {
                let _ = kill_process_group(pid, signal);
            }
        }
    }
    for process in alive {
        if let Some(pid) = Pid::from_raw(process.pid) {
            match kill_process(pid, signal) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(e) => {
                    return Err(StopError::Signal {
                        pid: process.pid,
                        source: e.into(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// The processes of `session` still alive under `proc_root`: same pid and
/// start time (so a reused pid isn't mistaken for it), or carrying the tag.
fn alive_in(proc_root: &Path, session: &Session) -> Vec<ProcessInfo> {
    list_in(proc_root)
        .into_iter()
        .find(|s| s.tag == session.tag)
        .map(|s| s.processes)
        .unwrap_or_default()
        .into_iter()
        .chain(session.processes.iter().filter_map(|p| {
            let (now, _) = read_process(&proc_root.join(p.pid.to_string()), p.pid)?;
            // A zombie still has a stat; only exited-and-reaped is gone.
            (now.start == p.start && !is_zombie(proc_root, p.pid)).then_some(now)
        }))
        .fold(Vec::new(), |mut all, p| {
            if !all.iter().any(|q: &ProcessInfo| q.pid == p.pid) && !is_zombie(proc_root, p.pid) {
                all.push(p);
            }
            all
        })
}

fn is_zombie(proc_root: &Path, pid: i32) -> bool {
    fs::read_to_string(proc_root.join(pid.to_string()).join("stat"))
        .ok()
        .and_then(|text| {
            text.rfind(')')
                .map(|i| text[i + 1..].trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

/// Stops `session`: `SIGTERM` to its group and tagged processes, up to
/// `grace` for them to exit, then `SIGKILL`.
pub fn stop(session: &Session, grace: Duration) -> Result<Stopped, StopError> {
    stop_in(Path::new("/proc"), session, grace)
}

fn wait_gone(proc_root: &Path, session: &Session, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if alive_in(proc_root, session).is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub(crate) fn stop_in(
    proc_root: &Path,
    session: &Session,
    grace: Duration,
) -> Result<Stopped, StopError> {
    let alive = alive_in(proc_root, session);
    if alive.is_empty() {
        return Ok(Stopped::Terminated);
    }
    signal_session(session, &alive, Signal::TERM)?;
    if wait_gone(proc_root, session, grace) {
        return Ok(Stopped::Terminated);
    }
    let alive = alive_in(proc_root, session);
    signal_session(session, &alive, Signal::KILL)?;
    if wait_gone(proc_root, session, Duration::from_secs(5)) {
        Ok(Stopped::Killed)
    } else {
        Err(StopError::Stuck(session.tag.clone()))
    }
}
