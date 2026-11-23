//! `profiles list|show|new|edit|edit-games|delete`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use libsruman::{
    model::{self, Profile, SlrMode},
    prefix::{self, PrefixState},
    process,
    version::{BuildId, RuntimeRef},
};
use serde::Serialize;

use super::games;
use crate::app::{App, AppError};

#[derive(Debug, Serialize)]
struct ProfileRow {
    name: String,
    runtime: String,
    mode: String,
    prefix: Option<PathBuf>,
    implicit: bool,
    in_use: bool,
}

/// Whether a game or session runs with `profile` or in `prefix`.
fn in_use(app: &App, profile: &Profile) -> bool {
    let prefix = profile
        .prefix
        .as_deref()
        .map(|p| libsruman::paths::expand_tilde(p, app.home()));
    prefix
        .as_deref()
        .is_some_and(|p| prefix::in_use(&app.settings.dirs.app.locks, p))
        || process::list()
            .iter()
            .any(|s| s.profile.as_deref() == Some(profile.name.as_str()))
}

fn row(app: &App, profile: &Profile, implicit: bool) -> ProfileRow {
    let (runtime, mode) = profile.slr.as_ref().map_or_else(
        || ("-".to_owned(), "-".to_owned()),
        |slr| (slr.runtime.clone(), slr.mode.to_string()),
    );
    ProfileRow {
        name: profile.name.clone(),
        runtime,
        mode,
        prefix: profile
            .prefix
            .as_deref()
            .map(|p| libsruman::paths::expand_tilde(p, app.home())),
        implicit,
        in_use: in_use(app, profile),
    }
}

/// `profiles list`: files, then the implicit ones (one per runtime).
pub fn list(app: &mut App) -> Result<u8, AppError> {
    let profiles = app.profiles();
    for issue in &profiles.issues {
        app.out.warning(crate::app::issue_text(issue))?;
    }
    let mut rows: Vec<ProfileRow> = profiles
        .profiles
        .values()
        .map(|p| row(app, p, false))
        .collect();
    let implicit: Vec<Profile> = profiles
        .implicit_names(app.catalog())
        .filter_map(|name| profiles.get(name, app.catalog()))
        .collect();
    rows.extend(implicit.iter().map(|p| row(app, p, true)));
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
            let mut notes = Vec::new();
            if row.implicit {
                notes.push("implicit".to_owned());
            }
            if row.in_use {
                notes.push(app.out.green("in use"));
            }
            vec![
                app.out.bold(&row.name),
                row.runtime.clone(),
                row.mode.clone(),
                row.prefix
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), |p| p.display().to_string()),
                notes.join(", "),
            ]
        })
        .collect();
    app.out.table(&table)?;
    Ok(0)
}

#[derive(Debug, Serialize)]
struct ShowProfile {
    name: String,
    file: Option<PathBuf>,
    runtime: String,
    build: Option<BuildId>,
    mode: String,
    prefix: Option<PathBuf>,
    prefix_state: Option<PrefixState>,
    shared_with: Vec<String>,
    in_use: bool,
    wrappers: Vec<String>,
}

/// `profiles show <P>`.
pub fn show(app: &mut App, name: &str) -> Result<u8, AppError> {
    let profiles = app.profiles();
    let profile = app.profile(&profiles, name)?;
    let file = profiles
        .profiles
        .contains_key(name)
        .then(|| profile_file(app, name));
    let row = row(app, &profile, file.is_none());
    let build = model::slr(&profile)
        .ok()
        .and_then(|(_, runtime)| app.store().resolve(&runtime).ok())
        .map(|b| b.build);
    let shared_with = profile.prefix.as_deref().map_or_else(Vec::new, |p| {
        prefix::sharing(p, &profiles, app.home())
            .into_iter()
            .filter(|other| *other != name)
            .map(str::to_owned)
            .collect()
    });
    let show = ShowProfile {
        name: row.name,
        file,
        runtime: row.runtime,
        build,
        mode: row.mode,
        prefix_state: row
            .prefix
            .as_deref()
            .filter(|p| p.is_dir())
            .map(prefix::read_state),
        prefix: row.prefix,
        shared_with,
        in_use: row.in_use,
        wrappers: profile.wrappers.clone(),
    };
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&show)?;
        return Ok(0);
    }
    let mut rows: Vec<(&str, String)> = vec![
        ("Name", show.name.clone()),
        (
            "File",
            show.file
                .as_ref()
                .map_or_else(|| "None (implicit)".into(), |f| f.display().to_string()),
        ),
        ("Runtime", show.runtime.clone()),
        (
            "Build",
            show.build
                .as_ref()
                .map_or_else(|| "not installed".into(), ToString::to_string),
        ),
        ("Mode", show.mode.clone()),
        (
            "Prefix",
            show.prefix
                .as_ref()
                .map_or_else(|| "None (real home)".into(), |p| p.display().to_string()),
        ),
    ];
    if show.prefix.is_some() {
        let state = match &show.prefix_state {
            None => "not created yet (`sruman init`)".to_owned(),
            Some(state) => {
                let uses: Vec<String> = state
                    .runtimes
                    .iter()
                    .map(|(rt, last)| format!("{rt} {} by {}", last.build, last.profile))
                    .collect();
                if uses.is_empty() {
                    "created".to_owned()
                } else {
                    format!("last used with {}", uses.join(", "))
                }
            }
        };
        rows.push(("Prefix State", state));
        rows.push((
            "Shared With",
            if show.shared_with.is_empty() {
                "None".into()
            } else {
                show.shared_with.join("  ")
            },
        ));
    }
    if !show.wrappers.is_empty() {
        rows.push(("Wrappers", show.wrappers.join("  ")));
    }
    rows.push(("In Use", if show.in_use { "Yes" } else { "No" }.into()));
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in rows {
        let key = app.out.bold(&format!("{key:<width$}"));
        app.out.line(format!("{key} : {value}"))?;
    }
    Ok(0)
}

fn profile_file(app: &App, name: &str) -> PathBuf {
    app.settings.dirs.app.profiles.join(format!("{name}.toml"))
}

/// `~/…` for paths in the home, as people write them.
fn tilde(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix(home) {
        Ok(rest) if !rest.as_os_str().is_empty() => Path::new("~").join(rest),
        _ => path.to_path_buf(),
    }
}

/// `profiles new <P> --runtime <rt> [--mode] [--prefix]`, then `init` when
/// the runtime is installed. Container profiles get
/// `<prefix_root>/<P>` without `--prefix`; overlay profiles run in the
/// real home unless given one.
pub fn new(
    app: &mut App,
    name: &str,
    runtime: &RuntimeRef,
    mode: SlrMode,
    prefix: Option<&Path>,
) -> Result<u8, AppError> {
    let file = profile_file(app, name);
    if file.exists() {
        return Err(AppError::Invalid(format!(
            "profile `{name}` exists already: {}; edit it with `sruman profiles edit {name}`",
            file.display()
        )));
    }
    let mut profile = model::new_profile(name, runtime);
    if let Some(slr) = profile.slr.as_mut() {
        slr.mode = mode;
    }
    let prefix = match (prefix, mode) {
        (Some(prefix), _) => Some(prefix.to_path_buf()),
        (None, SlrMode::Container) => Some(app.settings.dirs.prefix_root.join(name)),
        (None, SlrMode::Overlay) => None,
    };
    profile.prefix = prefix.as_deref().map(|p| tilde(p, app.home()));
    let issues = model::validate_profile(&profile, &app.validation());
    app.check_issues(&issues)?;
    let saved = model::save_profile(&app.settings.dirs.app.profiles, &profile)
        .map_err(|e| AppError::Failed(e.to_string()))?;
    tracing::info!(profile = name, file = %saved.display(), "profile created");
    app.out.info(format!("created {}", saved.display()))?;
    if profile.prefix.is_none() {
        return Ok(0);
    }
    if let Some(prefix) = &prefix
        && prefix.is_dir()
    {
        app.out
            .info(format!("using the existing prefix {}", prefix.display()))?;
    }
    if app.store().resolve(runtime).is_err() && !app.out.can_ask() && !app.settings.auto_install {
        app.out.warning(format!(
            "{runtime} is not installed; run `sruman install {runtime}`, then `sruman init {name}`"
        ))?;
        return Ok(0);
    }
    games::init(app, name)
}

/// Runs `$VISUAL`/`$EDITOR` (else `vi`) on `file`.
#[expect(
    clippy::disallowed_methods,
    reason = "`profiles edit` runs the user's editor; argv split on whitespace, no shell"
)]
fn run_editor(app: &App, file: &Path) -> Result<(), AppError> {
    let editor = app.settings.editor.clone().unwrap_or_else(|| "vi".into());
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or("vi");
    let status = Command::new(program)
        .args(words)
        .arg(file)
        .status()
        .map_err(|e| AppError::Failed(format!("can't run `{editor}`: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Failed(format!("`{editor}` exited with {status}")))
    }
}

/// `profiles edit <P>`: an implicit profile gets its file first.
pub fn edit(app: &mut App, name: &str) -> Result<u8, AppError> {
    let profiles = app.profiles();
    let profile = app.profile(&profiles, name)?;
    let file = profile_file(app, name);
    if !file.exists() {
        model::save_profile(&app.settings.dirs.app.profiles, &profile)
            .map_err(|e| AppError::Failed(e.to_string()))?;
    }
    run_editor(app, &file)?;
    let profiles = app.profiles();
    let mut issues: Vec<_> = profiles
        .issues
        .iter()
        .filter(|i| i.to_string().contains(&format!("{name}.toml")))
        .cloned()
        .collect();
    if let Some(profile) = profiles.profiles.get(name) {
        issues.extend(model::validate_profile(profile, &app.validation()));
    }
    app.check_issues(&issues)?;
    app.out.info(format!("{}: ok", file.display()))?;
    Ok(0)
}

/// `profiles edit-games`.
pub fn edit_games(app: &mut App) -> Result<u8, AppError> {
    let file = app.settings.dirs.app.games_file.clone();
    if !file.exists() {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::Failed(format!("{}: {e}", dir.display())))?;
        }
        std::fs::write(&file, GAMES_TEMPLATE)
            .map_err(|e| AppError::Failed(format!("{}: {e}", file.display())))?;
    }
    run_editor(app, &file)?;
    let games = app.games();
    let mut issues = games.issues.clone();
    issues.extend(games.validate(
        &app.profiles(),
        app.settings.default_profile.as_deref(),
        &app.validation(),
    ));
    app.check_issues(&issues)?;
    app.out.info(format!("{}: ok", file.display()))?;
    Ok(0)
}

const GAMES_TEMPLATE: &str = "\
# Games for `sruman run <name>`, in lootrs' format.
#
# [[game]]
# name = \"hollow-knight\"
# exe = \"~/Games/HollowKnight/hollow_knight.x86_64\"
# profile = \"scout\"
";

/// `profiles delete <P>… [--prefix]`.
pub fn delete(app: &mut App, names: &[String], with_prefix: bool) -> Result<u8, AppError> {
    let profiles = app.profiles();
    for name in names {
        if !profiles.profiles.contains_key(name) {
            let implicit = profiles.get(name, app.catalog()).is_some();
            return Err(AppError::Invalid(if implicit {
                format!("`{name}` is an implicit profile, with no file to delete")
            } else {
                format!("no profile named `{name}`")
            }));
        }
    }
    let mut code = 0;
    for name in names {
        let profile = &profiles.profiles[name];
        if with_prefix && let Some(prefix) = &profile.prefix {
            let dir = libsruman::paths::expand_tilde(prefix, app.home());
            let others: Vec<&str> = prefix::sharing(prefix, &profiles, app.home())
                .into_iter()
                .filter(|other| !names.iter().any(|n| n == other))
                .collect();
            if !others.is_empty() {
                app.out.warning(format!(
                    "keeping {}: profile{} {} use{} it too",
                    dir.display(),
                    if others.len() == 1 { "" } else { "s" },
                    others.join(", "),
                    if others.len() == 1 { "s" } else { "" }
                ))?;
            } else if dir.exists() {
                let question =
                    format!("Delete the prefix {} with everything in it?", dir.display());
                if app.out.confirm(&question, true)? {
                    if let Err(err) = prefix::remove(&dir, &app.prefix_ctx()) {
                        let err = AppError::from(err);
                        code = code.max(err.exit_code());
                        app.out.error(err.to_string())?;
                        continue;
                    }
                    tracing::info!(prefix = %dir.display(), "prefix deleted");
                    app.out.info(format!("deleted {}", dir.display()))?;
                }
            }
        }
        let file = profile_file(app, name);
        std::fs::remove_file(&file)
            .map_err(|e| AppError::Failed(format!("{}: {e}", file.display())))?;
        tracing::info!(profile = name, "profile deleted");
        app.out.info(format!("deleted {}", file.display()))?;
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}
