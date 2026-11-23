//! Launching and supervising games, executables and shells (4.5).
//!
//! The supervisor owns the run log: header, every output line with a time
//! stamp, and the exit status. `run`/`execute` without `--wait` re-execute
//! sruman as a detached supervisor (its own session, hidden
//! `--supervised`), which reports one status line back over its stdout:
//! `ok\t<pid>\t<log>`, or `error\t<exit code>` and the message.
//!
//! Locks: the store lock is never held here (an install before a launch
//! drops it first), so the game can't inherit it; flocks follow the open
//! file, and a game holding `.lock` would block every install while it
//! runs. Only the build holds and the prefix lock from
//! [`libsruman::launch::LaunchPlan::acquire`] are passed on, on purpose.

#![allow(
    clippy::disallowed_methods,
    reason = "the supervisor starts games and shells"
)]

use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use jiff::{ZonedDateTime, fmt::strtime};
use libsruman::{
    catalog::Entry,
    launch::{self, LaunchError, LaunchSpec, Tag, Target},
    model::{self, Game, Profile, SlrMode},
    process,
    store::Installed,
    version::RuntimeRef,
};

use super::{App, AppError};
use crate::{
    config::LogLevel,
    settings::{
        Command as Cmd, Settings,
        command::{Execute, LaunchMode, Run},
    },
};

pub mod runlog;

use runlog::RunKind;

/// How long to keep reading output after the game exits: its children may
/// hold the pipes open much longer.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Run-log names, `YYYYMMDD-HHMMSS`, sort by time.
pub const STAMP: &str = "%Y%m%d-%H%M%S";

/// What to launch.
#[derive(Debug, Clone, Copy)]
enum Request<'a> {
    Game(&'a Game),
    Execute { profile: &'a str, target: &'a Path },
}

impl Request<'_> {
    fn describe(self) -> String {
        match self {
            Self::Game(game) => format!("`{}`", game.name),
            Self::Execute { profile, target } => {
                format!("`{}` with profile `{profile}`", target.display())
            }
        }
    }

    fn title(self) -> String {
        match self {
            Self::Game(game) => format!("run {}", game.name),
            Self::Execute { target, .. } => format!("execute {}", target.display()),
        }
    }
}

/// A started game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub pid: u32,
    pub log: PathBuf,
}

/// `run <game>`.
pub fn run(app: &mut App, run: &Run) -> Result<u8, AppError> {
    let games = app.games();
    let game = games.get(&run.game).cloned().ok_or_else(|| {
        AppError::Invalid(format!(
            "no game named `{}` in {}",
            run.game,
            app.settings.dirs.app.games_file.display()
        ))
    })?;
    let default_profile = app.settings.default_profile.as_deref();
    let name = run
        .profile
        .as_deref()
        .or_else(|| game.profile_name(default_profile))
        .ok_or_else(|| {
            AppError::Invalid(format!(
                "game `{}` has no profile: set `profile` in games.toml or `default_profile` in config.toml, or pass -p",
                game.name
            ))
        })?
        .to_owned();
    let profile = app.profile(&app.profiles(), &name)?;
    launch(
        app,
        Request::Game(&game),
        &profile,
        run.mode,
        run.debug,
        &run.args,
    )
}

/// `execute -p <profile> <path|program>`.
pub fn execute(app: &mut App, execute: &Execute) -> Result<u8, AppError> {
    let name = execute
        .profile
        .as_deref()
        .ok_or_else(|| AppError::Invalid("execute needs a profile".into()))?;
    let profile = app.profile(&app.profiles(), name)?;
    launch(
        app,
        Request::Execute {
            profile: name,
            target: &execute.target,
        },
        &profile,
        execute.mode,
        execute.debug,
        &execute.args,
    )
}

fn launch(
    app: &mut App,
    request: Request,
    profile: &Profile,
    mode: LaunchMode,
    debug: bool,
    args: &[String],
) -> Result<u8, AppError> {
    match mode {
        LaunchMode::Detach => {
            prelaunch(app, request, profile)?;
            let started = spawn_detached(app.settings)?;
            app.out.info(format!(
                "started {} (pid {})",
                request.describe(),
                started.pid
            ))?;
            app.out.info(format!("log: {}", started.log.display()))?;
            Ok(0)
        }
        LaunchMode::Wait => {
            prelaunch(app, request, profile)?;
            supervise(app, request, profile, debug, args, true, |_| {})
        }
        LaunchMode::Supervised => {
            // Leave the terminal's session, so closing it leaves the game be.
            if let Err(err) = rustix::process::setsid() {
                tracing::warn!("setsid failed: {err}");
            }
            let mut reported = false;
            let result = supervise(app, request, profile, debug, args, false, |started| {
                // The parent stops reading after this line.
                let mut status = io::stdout();
                let _ = writeln!(status, "ok\t{}\t{}", started.pid, started.log.display());
                let _ = status.flush();
                reported = true;
            });
            match result {
                // Before "ok" the parent still listens and reports it.
                Err(err) if !reported => {
                    let mut status = io::stdout();
                    let _ = writeln!(status, "error\t{}\n{err}", err.exit_code());
                    let _ = status.flush();
                    Ok(0)
                }
                // Nobody sees a detached supervisor's exit code.
                result => result.map(|_| 0),
            }
        }
    }
}

/// Before a launch, in the foreground: install a missing runtime (D12),
/// check the game and profile, and refuse a game that already runs.
fn prelaunch(app: &mut App, request: Request, profile: &Profile) -> Result<(), AppError> {
    ensure_runtime(app, profile)?;
    let mut issues = Vec::new();
    if let Request::Game(game) = request {
        issues.extend(game.validate(Some(profile), &app.validation()));
    }
    let store = app.store();
    issues.extend(launch::preflight(
        profile,
        &app.launch_ctx(&store, false, &[]),
    ));
    app.check_issues(&issues)?;
    if let Request::Game(game) = request {
        refuse_running(&game.name)?;
    }
    Ok(())
}

fn refuse_running(game: &str) -> Result<(), AppError> {
    match process::find_game(game) {
        Some(session) => Err(AppError::Busy(format!(
            "`{game}` is already running (pid {}); stop it first with `sruman stop {game}`",
            session.leader.pid
        ))),
        None => Ok(()),
    }
}

/// D12: a profile's runtime (and scout's container) that isn't installed
/// is installed after a `[Y/n]` on a terminal, or right away with
/// `auto_install`; otherwise exit 2 naming the command.
pub fn ensure_runtime(app: &mut App, profile: &Profile) -> Result<(), AppError> {
    let missing = missing_runtimes(app, profile)?;
    install_missing(app, &missing, &format!("profile `{}`", profile.name))
}

/// D12 for `missing` runtimes that `who` needs: install them after a
/// prompt or with `auto_install`, else exit 2 naming the command.
#[cfg_attr(not(feature = "download"), allow(unused_variables))]
pub fn install_missing(app: &mut App, missing: &[RuntimeRef], who: &str) -> Result<(), AppError> {
    if missing.is_empty() {
        return Ok(());
    }
    let names = missing
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(feature = "download")]
    {
        let install = app.settings.auto_install
            || (app.out.can_ask()
                && app
                    .out
                    .confirm(&format!("{names}: not installed. Install it now?"), true)?);
        if install {
            let fetch = libsruman::fetch::UreqFetch::new();
            super::commands::runtimes::install_with(app, &fetch, missing, true, false, false)?;
            return Ok(());
        }
    }
    Err(AppError::Invalid(format!(
        "{who} needs {names}, which is not installed; install it with `sruman install {names}`"
    )))
}

/// The runtimes `profile` needs that have no installed build.
fn missing_runtimes(app: &App, profile: &Profile) -> Result<Vec<RuntimeRef>, AppError> {
    let Ok((slr, runtime)) = model::slr(profile) else {
        return Ok(Vec::new()); // validation reports it
    };
    let Some(def) = app.catalog().get(&runtime.name) else {
        return Ok(Vec::new());
    };
    let store = app.store();
    let mut missing = Vec::new();
    if store.resolve(&runtime).is_err() {
        missing.push(runtime.clone());
    }
    if def.entry == Entry::ScoutOnSoldier && slr.mode == SlrMode::Container {
        for dep in &def.depends {
            if store.installed_of(dep)?.is_empty() {
                missing.push(RuntimeRef {
                    name: dep.clone(),
                    build: None,
                });
            }
        }
    }
    Ok(missing)
}

/// Prepares, locks, spawns and waits; everything goes into the run log.
fn supervise(
    app: &mut App,
    request: Request,
    profile: &Profile,
    debug: bool,
    extra_args: &[String],
    tee: bool,
    on_started: impl FnOnce(&Started),
) -> Result<u8, AppError> {
    let settings = app.settings;
    let store = app.store();
    let ctx = app.launch_ctx(&store, debug, extra_args);
    let started_at = ZonedDateTime::now();
    let stamp = time(STAMP, &started_at);
    let (target, tag, kind) = match request {
        Request::Game(game) => (
            Target::Game(game),
            Tag::Game(game.name.clone()),
            RunKind::Game(&game.name),
        ),
        Request::Execute { profile, target } => (
            Target::Exe(target),
            Tag::Session(format!("execute:{profile}:{stamp}")),
            RunKind::Execute(profile),
        ),
    };
    let plan = launch::prepare(target, profile, &tag, &ctx)?;

    // Held until the game's first process exists (tagged from its exec
    // on), so two launches at once can't both pass the check.
    let launching = match request {
        Request::Game(game) => Some(game_lock(&settings.dirs.app.locks, &game.name)?),
        Request::Execute { .. } => None,
    };
    if let Request::Game(game) = request {
        refuse_running(&game.name)?;
    }

    let (file, log_path) =
        runlog::create(&settings.logging.dir, kind, &stamp, settings.logging.keep)
            .map_err(|e| AppError::Failed(format!("creating the run log: {e}")))?;
    let log = Arc::new(Mutex::new(file));

    let launched = match plan.acquire(&store, &app.prefix_ctx()) {
        Ok(launched) => launched,
        Err(err) => {
            let header = header(request, profile, &plan.builds, None, &started_at);
            let _ = write_log(&log, &format!("{header}=== not launched ===\n{err}\n"));
            return Err(acquire_error(err));
        }
    };
    write_log(
        &log,
        &header(
            request,
            profile,
            &plan.builds,
            Some(&launched.spec),
            &started_at,
        ),
    )
    .map_err(|e| AppError::Failed(format!("writing the run log: {e}")))?;

    // Detached, the game gets its own process group: `stop` signals the
    // game's group, and the supervisor must live on to write the footer.
    // With `--wait` it stays in sruman's, so Ctrl-C reaches it.
    let own_group = !tee;
    let spawned = launched
        .make_inheritable()
        .and_then(|()| spawn(&launched.spec, own_group));
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            let _ = write_log(&log, &format!("=== spawn error: {err} ===\n"));
            return Err(AppError::Failed(format!(
                "failed to start {}: {err}",
                launched.spec.program.display()
            )));
        }
    };
    // Only the game keeps the holds and the prefix lock now.
    drop(launched);
    drop(launching);
    let started = Started {
        pid: child.id(),
        log: log_path,
    };
    tracing::info!(pid = started.pid, log = %started.log.display(), "started {}", request.describe());
    on_started(&started);

    let clock = Instant::now();
    let drained = pump_output(&mut child, &log, tee);
    let status = child
        .wait()
        .map_err(|e| AppError::Failed(format!("waiting for the game: {e}")))?;
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if drained.recv_timeout(left).is_err() {
            break;
        }
    }
    let footer = footer(status, clock.elapsed());
    tracing::info!(
        "{} {}",
        request.describe(),
        footer.trim_matches(['=', ' ', '\n'])
    );
    let _ = write_log(&log, &footer);
    Ok(exit_code(status))
}

/// `acquire`'s problems: a build being removed or a prefix in use is busy
/// (exit 4), anything else a failure.
fn acquire_error(err: LaunchError) -> AppError {
    let busy = err
        .problems
        .iter()
        .any(|p| p.contains("is in use") || p.contains("is locked"));
    let message = format!("not launching:\n{}", super::indent(&err.problems));
    if busy {
        AppError::Busy(message)
    } else {
        AppError::Failed(message)
    }
}

/// An exclusive lock per game while it starts.
fn game_lock(locks: &Path, game: &str) -> Result<File, AppError> {
    std::fs::create_dir_all(locks)
        .map_err(|e| AppError::Failed(format!("{}: {e}", locks.display())))?;
    let path = locks.join(format!("game-{game}.lock"));
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| AppError::Failed(format!("{}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(AppError::Busy(format!(
            "`{game}` is being started by another sruman"
        ))),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(AppError::Failed(format!("{}: {e}", path.display())))
        }
    }
}

fn spawn(spec: &LaunchSpec, own_group: bool) -> io::Result<Child> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(&spec.program);
    if own_group {
        command.process_group(0);
    }
    command
        .args(&spec.args)
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Copies stdout and stderr line by line into the log (and with `tee` to
/// the terminal); each reader signals on the channel at EOF.
fn pump_output(child: &mut Child, log: &Arc<Mutex<File>>, tee: bool) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    let streams: [(Box<dyn Read + Send>, &str); 2] = [
        (
            Box::new(child.stdout.take().expect("stdout is piped")),
            "stdout",
        ),
        (
            Box::new(child.stderr.take().expect("stderr is piped")),
            "stderr",
        ),
    ];
    for (reader, label) in streams {
        let log = Arc::clone(log);
        let tx = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\n', '\r']);
                let stamp = time("%H:%M:%S%.3f", &ZonedDateTime::now());
                let _ = write_log(&log, &format!("[{stamp}] [{label}] {text}\n"));
                if tee {
                    let _ = if label == "stdout" {
                        writeln!(io::stdout(), "{text}")
                    } else {
                        writeln!(io::stderr(), "{text}")
                    };
                }
                line.clear();
            }
            let _ = tx.send(());
        });
    }
    rx
}

fn write_log(log: &Mutex<File>, text: &str) -> io::Result<()> {
    let mut file = log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    file.write_all(text.as_bytes())
}

/// `at` in a fixed strftime `format` (sruman's are all valid).
pub fn time(format: &str, at: &ZonedDateTime) -> String {
    strtime::format(format, at).unwrap_or_default()
}

/// Run-log header: what runs, how, on which runtime and system.
fn header(
    request: Request,
    profile: &Profile,
    builds: &[Installed],
    spec: Option<&LaunchSpec>,
    started_at: &ZonedDateTime,
) -> String {
    let mut out = format!(
        "=== sruman {} — {} — {} ===\n",
        env!("CARGO_PKG_VERSION"),
        request.title(),
        time("%Y-%m-%dT%H:%M:%S%:z", started_at),
    );
    if let Request::Game(game) = request {
        out += &format!(
            "--- game ---\n{}",
            toml::to_string(game).unwrap_or_default()
        );
    }
    out += &format!(
        "--- profile ---\n{}",
        toml::to_string(profile).unwrap_or_default()
    );
    out += "--- runtime ---\n";
    for build in builds {
        out += &format!("{} {} ({})\n", build.name, build.build, build.dir.display());
    }
    if let Some(spec) = spec {
        out += &format!("--- command ---\n{}\n", command_line(spec.argv()));
        out += &format!("cwd: {}\n", spec.cwd.display());
        out += "--- env (on top of inherited) ---\n";
        for (key, value) in &spec.env {
            out += &format!("{key}={}\n", value.to_string_lossy());
        }
    }
    out += &format!(
        "--- system ---\nkernel: {}\ndistro: {}\n",
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map_or_else(|_| "unknown".to_owned(), |k| k.trim().to_owned()),
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("PRETTY_NAME=")
                        .map(|v| v.trim_matches('"').to_owned())
                })
            })
            .unwrap_or_else(|| "unknown".to_owned()),
    );
    if spec.is_some() {
        out += "=== output ===\n";
    }
    out
}

/// The argv as a shell would need it typed.
pub fn command_line<'a>(argv: impl Iterator<Item = &'a std::ffi::OsStr>) -> String {
    argv.map(|arg| quote(&arg.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%".contains(c));
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

pub fn footer(status: ExitStatus, elapsed: Duration) -> String {
    use std::os::unix::process::ExitStatusExt;

    let secs = elapsed.as_secs();
    let duration = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    let exit = match (status.code(), status.signal()) {
        (Some(code), _) => format!("code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "unknown".into(),
    };
    format!("=== exit: {exit} — duration {duration} ===\n")
}

/// Shell convention: the exit code, or 128 + the signal.
pub fn exit_code(status: ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;

    match (status.code(), status.signal()) {
        (Some(code), _) => u8::try_from(code).unwrap_or(1),
        (None, Some(signal)) => u8::try_from(128 + signal).unwrap_or(255),
        (None, None) => 1,
    }
}

/// Re-executes sruman as a detached supervisor and waits for its status.
fn spawn_detached(settings: &Settings) -> Result<Started, AppError> {
    let failed = |e: io::Error| AppError::Failed(format!("failed to start the supervisor: {e}"));
    let exe = std::env::current_exe().map_err(failed)?;
    let mut child = Command::new(exe)
        .args(supervised_args(settings))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failed)?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let status = read_status(stdout);
    // Reap it if it exited already (the error case); otherwise it's
    // reparented when sruman exits.
    let _ = child.try_wait();
    status
}

/// Parses the supervisor's report: the first `ok`/`error` line.
pub fn read_status(stdout: impl Read) -> Result<Started, AppError> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| AppError::Failed(format!("reading the supervisor's status: {e}")))?;
        if read == 0 {
            return Err(AppError::Failed(
                "the supervisor exited without reporting; see the app log".into(),
            ));
        }
        let mut fields = line.trim_end_matches('\n').split('\t');
        match (fields.next(), fields.next(), fields.next()) {
            (Some("ok"), Some(pid), Some(log)) => {
                return Ok(Started {
                    pid: pid.parse().unwrap_or_default(),
                    log: log.into(),
                });
            }
            (Some("error"), Some(code), None) => {
                let mut message = String::new();
                let _ = reader.read_to_string(&mut message);
                return Err(AppError::Supervisor {
                    code: code.parse().unwrap_or(1),
                    message: message.trim_end().to_owned(),
                });
            }
            _ => {}
        }
    }
}

/// The command line that repeats this `run`/`execute` as a supervisor.
pub fn supervised_args(settings: &Settings) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if settings.dirs.config_file.is_file() {
        args.extend(["--config".into(), settings.dirs.config_file.clone().into()]);
    }
    match settings.logging.stderr {
        Some(LogLevel::Trace) => args.push("-vv".into()),
        Some(_) => args.push("-v".into()),
        None => {}
    }
    let (debug, extra) = match &settings.command {
        Cmd::Run(run) => {
            args.extend(["run".into(), run.game.clone().into()]);
            if let Some(profile) = &run.profile {
                args.extend(["--profile".into(), profile.into()]);
            }
            (run.debug, &run.args)
        }
        Cmd::Execute(execute) => {
            args.push("execute".into());
            if let Some(profile) = &execute.profile {
                args.extend(["--profile".into(), profile.into()]);
            }
            args.push(execute.target.clone().into());
            (execute.debug, &execute.args)
        }
        other => unreachable!("`{}` has no supervisor", other.name()),
    };
    args.push("--supervised".into());
    if debug {
        args.push("--debug".into());
    }
    if !extra.is_empty() {
        args.push("--".into());
        args.extend(extra.iter().map(OsString::from));
    }
    args
}

/// `shell [-p <profile>]`: an interactive shell in the profile's runtime
/// and prefix, on this terminal; no run log.
pub fn shell(app: &mut App, name: &str, debug: bool) -> Result<u8, AppError> {
    let profile = app.profile(&app.profiles(), name)?;
    ensure_runtime(app, &profile)?;
    let store = app.store();
    let issues = launch::preflight(&profile, &app.launch_ctx(&store, false, &[]));
    app.check_issues(&issues)?;
    let stamp = time(STAMP, &ZonedDateTime::now());
    let tag = Tag::Session(format!("shell:{name}:{stamp}"));
    let plan = launch::prepare(
        Target::Shell,
        &profile,
        &tag,
        &app.launch_ctx(&store, debug, &[]),
    )?;
    let launched = plan
        .acquire(&store, &app.prefix_ctx())
        .map_err(acquire_error)?;
    let spec = &launched.spec;
    tracing::info!(profile = name, "shell: {}", command_line(spec.argv()));
    let child = launched.make_inheritable().and_then(|()| {
        Command::new(&spec.program)
            .args(&spec.args)
            .envs(&spec.env)
            .current_dir(&spec.cwd)
            .spawn()
    });
    let mut child = child.map_err(|e| {
        AppError::Failed(format!("failed to start {}: {e}", spec.program.display()))
    })?;
    drop(launched);
    let status = child
        .wait()
        .map_err(|e| AppError::Failed(format!("waiting for the shell: {e}")))?;
    Ok(exit_code(status))
}

#[cfg(test)]
mod tests;
