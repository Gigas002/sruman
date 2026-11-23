//! Games and running: `status`, `stop`, `killall`, `list`, `show`, `check`,
//! `logs`, `init`.

use std::{collections::BTreeMap, path::PathBuf};

use libsruman::{
    launch::{self, Tag, Target},
    model::{self, Issue, Severity},
    prefix::{self, HomeMode, PrefixUse},
    process::{self, Session, SessionTag, Stopped},
    version::BuildId,
};
use serde::Serialize;

use crate::app::{
    App, AppError,
    supervisor::{self, runlog},
};

// --- status / stop ----------------------------------------------------------

#[derive(Debug, Serialize)]
struct SessionRow {
    /// `game` or `session`.
    kind: &'static str,
    name: String,
    pid: i32,
    processes: usize,
    profile: Option<String>,
    prefix: Option<PathBuf>,
}

fn session_row(session: &Session) -> SessionRow {
    let (kind, name) = match &session.tag {
        SessionTag::Game(name) => ("game", name.clone()),
        SessionTag::Session(name) => ("session", name.clone()),
    };
    SessionRow {
        kind,
        name,
        pid: session.leader.pid,
        processes: session.processes.len(),
        profile: session.profile.clone(),
        prefix: session.prefix.clone(),
    }
}

/// `status`: running games and sessions.
pub fn status(app: &mut App) -> Result<u8, AppError> {
    let rows: Vec<SessionRow> = process::list().iter().map(session_row).collect();
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&rows)?;
        return Ok(0);
    }
    if rows.is_empty() {
        app.out.info("nothing is running")?;
        return Ok(0);
    }
    let mut table = Vec::new();
    for row in &rows {
        table.push(vec![
            app.out.bold(&row.name),
            format!("pid {}", row.pid),
            format!(
                "{} process{}",
                row.processes,
                if row.processes == 1 { "" } else { "es" }
            ),
            row.profile
                .as_deref()
                .map_or_else(String::new, |p| format!("profile {p}")),
            row.prefix
                .as_ref()
                .map_or_else(String::new, |p| p.display().to_string()),
        ]);
    }
    app.out.table(&table)?;
    Ok(0)
}

/// The sessions `target` names: a game, a profile, a session or a pid.
fn matching<'a>(sessions: &'a [Session], target: &str) -> Vec<&'a Session> {
    let pid: Option<i32> = target.parse().ok();
    sessions
        .iter()
        .filter(|s| {
            let name = match &s.tag {
                SessionTag::Game(name) | SessionTag::Session(name) => name,
            };
            name == target
                || s.profile.as_deref() == Some(target)
                || pid.is_some_and(|pid| s.processes.iter().any(|p| p.pid == pid))
        })
        .collect()
}

/// `stop <game|profile|pid>…`.
pub fn stop(app: &mut App, targets: &[String]) -> Result<u8, AppError> {
    let sessions = process::list();
    let mut code = 0;
    for target in targets {
        let found = matching(&sessions, target);
        if found.is_empty() {
            code = code.max(3);
            app.out.error(format!("`{target}` is not running"))?;
            continue;
        }
        for session in found {
            code = code.max(stop_one(app, session)?);
        }
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}

/// `killall`: every session sruman started.
pub fn killall(app: &mut App) -> Result<u8, AppError> {
    let sessions = process::list();
    if sessions.is_empty() {
        app.out.info("nothing is running")?;
        return Ok(0);
    }
    let mut code = 0;
    for session in &sessions {
        code = code.max(stop_one(app, session)?);
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}

fn stop_one(app: &mut App, session: &Session) -> Result<u8, AppError> {
    match process::stop(session, app.settings.stop_timeout) {
        Ok(how) => {
            let how = match how {
                Stopped::Terminated => "stopped",
                Stopped::Killed => "killed",
            };
            tracing::info!("{how} {}", session.tag);
            app.out.info(format!("{how} {}", session.tag))?;
            Ok(0)
        }
        Err(err) => {
            app.out.error(format!("{}: {err}", session.tag))?;
            Ok(1)
        }
    }
}

// --- list / show --------------------------------------------------------------

#[derive(Debug, Serialize)]
struct GameRow {
    name: String,
    profile: Option<String>,
    runtime: Option<String>,
    running: bool,
}

/// `list`: games, with their profile, runtime and whether they run.
pub fn list(app: &mut App) -> Result<u8, AppError> {
    let games = app.games();
    let profiles = app.profiles();
    for issue in &games.issues {
        app.out.warning(crate::app::issue_text(issue))?;
    }
    let running: Vec<String> = process::list()
        .into_iter()
        .filter_map(|s| match s.tag {
            SessionTag::Game(name) => Some(name),
            SessionTag::Session(_) => None,
        })
        .collect();
    let default_profile = app.settings.default_profile.as_deref();
    let rows: Vec<GameRow> = games
        .games
        .iter()
        .map(|game| {
            let profile = game.profile_name(default_profile).map(str::to_owned);
            let runtime = profile
                .as_deref()
                .and_then(|p| profiles.get(p, app.catalog()))
                .and_then(|p| p.slr.map(|slr| slr.runtime));
            GameRow {
                running: running.contains(&game.name),
                name: game.name.clone(),
                profile,
                runtime,
            }
        })
        .collect();
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&rows)?;
        return Ok(0);
    }
    if app.out.quiet {
        for row in &rows {
            app.out.line(&row.name)?;
        }
        return Ok(0);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                app.out.bold(&row.name),
                row.profile.clone().unwrap_or_else(|| "-".into()),
                row.runtime.clone().unwrap_or_else(|| "-".into()),
                if row.running {
                    app.out.green("running")
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    app.out.table(&table)?;
    Ok(0)
}

#[derive(Debug, Serialize)]
struct Show {
    name: String,
    exe: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    profile: String,
    runtimes: Vec<(String, BuildId)>,
    prefix: Option<PathBuf>,
    command: Option<String>,
    env: BTreeMap<String, String>,
    problems: Vec<String>,
    running: Option<i32>,
    latest_log: Option<PathBuf>,
}

/// `show <game>`: how it would run.
pub fn show(app: &mut App, name: &str) -> Result<u8, AppError> {
    let games = app.games();
    let game = games
        .get(name)
        .cloned()
        .ok_or_else(|| AppError::Invalid(format!("no game named `{name}` in games.toml")))?;
    let profile_name = game
        .profile_name(app.settings.default_profile.as_deref())
        .ok_or_else(|| AppError::Invalid(format!("game `{name}` has no profile")))?
        .to_owned();
    let profile = app.profile(&app.profiles(), &profile_name)?;
    let store = app.store();
    let ctx = app.launch_ctx(&store, false, &[]);
    let plan = launch::prepare(
        Target::Game(&game),
        &profile,
        &Tag::Game(game.name.clone()),
        &ctx,
    );
    let mut show = Show {
        name: game.name.clone(),
        exe: libsruman::paths::expand_tilde(&game.exe, app.home()),
        args: game.args.clone(),
        cwd: None,
        profile: profile_name,
        runtimes: Vec::new(),
        // The profile's, also when the launch can't be prepared (a runtime
        // that isn't installed): it's no less the game's prefix.
        prefix: profile
            .prefix
            .as_deref()
            .map(|p| libsruman::paths::expand_tilde(p, app.home())),
        command: None,
        env: BTreeMap::new(),
        problems: Vec::new(),
        running: process::find_game(&game.name).map(|s| s.leader.pid),
        latest_log: runlog::latest(&app.settings.logging.dir, runlog::RunKind::Game(name)),
    };
    match &plan {
        Ok(plan) => {
            let spec = plan.spec(&[]);
            show.cwd = Some(spec.cwd.clone());
            show.command = Some(supervisor::command_line(spec.argv()));
            show.env = spec
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string_lossy().into_owned()))
                .collect();
            show.runtimes = plan
                .builds
                .iter()
                .map(|b| (b.name.clone(), b.build.clone()))
                .collect();
            if let Some(prefix) = &plan.prefix {
                show.prefix = Some(prefix.dir.clone());
            }
        }
        Err(err) => show.problems.clone_from(&err.problems),
    }
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&show)?;
        return Ok(0);
    }
    let mut rows: Vec<(&str, String)> = vec![
        ("Name", show.name.clone()),
        ("Executable", show.exe.display().to_string()),
        ("Arguments", show.args.join(" ")),
        ("Profile", show.profile.clone()),
    ];
    for (name, build) in &show.runtimes {
        rows.push(("Runtime", format!("{name} {build}")));
    }
    rows.push((
        "Prefix",
        show.prefix
            .as_ref()
            .map_or_else(|| "None (real home)".into(), |p| p.display().to_string()),
    ));
    if let Some(cwd) = &show.cwd {
        rows.push(("Working Dir", cwd.display().to_string()));
    }
    if let Some(command) = &show.command {
        rows.push(("Command", command.clone()));
    }
    rows.push((
        "Running",
        show.running
            .map_or_else(|| "No".into(), |pid| format!("Yes (pid {pid})")),
    ));
    rows.push((
        "Latest Log",
        show.latest_log
            .as_ref()
            .map_or_else(|| "None".into(), |p| p.display().to_string()),
    ));
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in rows {
        let key = app.out.bold(&format!("{key:<width$}"));
        app.out.line(format!("{key} : {value}"))?;
    }
    if !show.env.is_empty() {
        app.out.line(app.out.bold("Environment"))?;
        for (key, value) in &show.env {
            app.out.line(format!("  {key}={value}"))?;
        }
    }
    for problem in &show.problems {
        app.out.warning(problem)?;
    }
    Ok(0)
}

// --- check ----------------------------------------------------------------------

/// `check [game|profile…]`: validation and preflight, without launching;
/// everything without arguments. Exit 2 on errors.
pub fn check(app: &mut App, targets: &[String]) -> Result<u8, AppError> {
    let games = app.games();
    let profiles = app.profiles();
    let store = app.store();
    let ctx = app.launch_ctx(&store, false, &[]);
    let validation = app.validation();
    let default_profile = app.settings.default_profile.as_deref();
    let mut issues: Vec<Issue> = Vec::new();
    let mut checked: Vec<String> = Vec::new();

    let check_profile = |name: &str, issues: &mut Vec<Issue>, checked: &mut Vec<String>| {
        if checked.iter().any(|c| c == name) {
            return;
        }
        checked.push(name.to_owned());
        if let Some(profile) = profiles.get(name, ctx.catalog) {
            issues.extend(launch::preflight(&profile, &ctx));
        }
    };

    if targets.is_empty() {
        issues.extend(games.issues.iter().cloned());
        issues.extend(profiles.issues.iter().cloned());
        issues.extend(games.validate(&profiles, default_profile, &validation));
        let names: Vec<String> = profiles.profiles.keys().cloned().collect();
        for name in &names {
            check_profile(name, &mut issues, &mut checked);
        }
        for game in &games.games {
            if let Some(name) = game.profile_name(default_profile) {
                check_profile(name, &mut issues, &mut checked);
            }
        }
    } else {
        for target in targets {
            if let Some(game) = games.get(target) {
                let profile = game
                    .profile_name(default_profile)
                    .and_then(|p| profiles.get(p, ctx.catalog));
                issues.extend(game.validate(profile.as_ref(), &validation));
                if let Some(name) = game.profile_name(default_profile) {
                    check_profile(name, &mut issues, &mut checked);
                }
            } else if profiles.get(target, ctx.catalog).is_some() {
                check_profile(target, &mut issues, &mut checked);
            } else {
                return Err(AppError::Invalid(format!(
                    "`{target}` is neither a game nor a profile"
                )));
            }
        }
    }

    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    let warnings = issues.len() - errors;
    for issue in &issues {
        match issue.severity {
            Severity::Error => app.out.error(crate::app::issue_text(issue))?,
            Severity::Warning => app.out.warning(crate::app::issue_text(issue))?,
        }
    }
    let summary = format!(
        "{} error{}, {} warning{}",
        errors,
        if errors == 1 { "" } else { "s" },
        warnings,
        if warnings == 1 { "" } else { "s" }
    );
    if errors > 0 {
        app.out.info(summary)?;
        return Ok(2);
    }
    app.out
        .info(format!("{} ({summary})", app.out.green("ok")))?;
    Ok(0)
}

// --- logs / init ----------------------------------------------------------------

/// `logs <game> [--path]`: the latest run log, or its path.
pub fn logs(app: &mut App, game: &str, path_only: bool) -> Result<u8, AppError> {
    let Some(path) = runlog::latest(&app.settings.logging.dir, runlog::RunKind::Game(game)) else {
        return Err(AppError::NotRunning(format!(
            "no run logs for `{game}` yet"
        )));
    };
    if path_only {
        app.out.line(path.display().to_string())?;
        return Ok(0);
    }
    let text =
        std::fs::read(&path).map_err(|e| AppError::Failed(format!("{}: {e}", path.display())))?;
    app.out.line(String::from_utf8_lossy(&text).trim_end())?;
    Ok(0)
}

/// `init <profile>`: create or update the profile's prefix.
pub fn init(app: &mut App, name: &str) -> Result<u8, AppError> {
    let profile = app.profile(&app.profiles(), name)?;
    let Some(prefix) = &profile.prefix else {
        return Err(AppError::Invalid(format!(
            "profile `{name}` has no prefix: it runs in your real home"
        )));
    };
    let prefix = libsruman::paths::expand_tilde(prefix, app.home());
    supervisor::ensure_runtime(app, &profile)?;
    let (slr, _) = model::slr(&profile).map_err(AppError::Invalid)?;
    let store = app.store();
    let mut problems = Vec::new();
    let runtimes =
        launch::resolve_runtimes(slr, &app.launch_ctx(&store, false, &[]), &mut problems);
    let Some(runtimes) = runtimes else {
        return Err(AppError::Invalid(problems.join("\n")));
    };
    let home = match runtimes.mode {
        model::SlrMode::Container => HomeMode::Container,
        model::SlrMode::Overlay => HomeMode::Env,
    };
    let build = runtimes.runtime.build.to_string();
    let prepared = prefix::ensure(
        &prefix,
        &PrefixUse {
            runtime: &runtimes.runtime.name,
            build: &build,
            profile: name,
            home,
        },
        &app.prefix_ctx(),
    )?;
    let what = if prepared.created {
        "created"
    } else {
        "up to date"
    };
    tracing::info!(profile = name, prefix = %prefix.display(), "init: {what}");
    app.out
        .info(format!("prefix {}: {what}", prefix.display()))?;
    Ok(0)
}
