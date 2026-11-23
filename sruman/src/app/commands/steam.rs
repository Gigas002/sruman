//! `steam status|link|unlink` (D17): Steam on sruman's runtimes.

use libsruman::{
    steam::{self, Slot, State, Steam, SteamError},
    store::Installed,
};

use super::super::{App, AppError};

impl From<SteamError> for AppError {
    fn from(err: SteamError) -> Self {
        match err {
            SteamError::Store(err) => err.into(),
            SteamError::Import(err) => err.into(),
            SteamError::Running | SteamError::InUse(_) => Self::Busy(err.to_string()),
            SteamError::Foreign { .. } => Self::Invalid(err.to_string()),
            SteamError::Io { .. } | SteamError::Damaged { .. } => Self::Failed(err.to_string()),
        }
    }
}

/// The Steam installation `steam_dir` names, or the one found in
/// `~/.steam/root` or `$XDG_DATA_HOME/Steam`.
fn locate(app: &App) -> Option<Steam> {
    let dirs = &app.settings.dirs;
    Steam::locate(dirs.steam_dir.as_deref(), &dirs.home, &dirs.data_home)
}

fn steam(app: &App) -> Result<Steam, AppError> {
    locate(app).ok_or_else(|| {
        let dirs = &app.settings.dirs;
        AppError::Invalid(match &dirs.steam_dir {
            Some(dir) => format!("no Steam at {} (`steam_dir`)", dir.display()),
            None => format!(
                "no Steam in ~/.steam/root or {}; set `steam_dir` in config.toml",
                dirs.data_home.join("Steam").display()
            ),
        })
    })
}

/// Builds that Steam links to, as `(why, build)`: `clean` keeps them and
/// `remove` refuses them. Nothing without a Steam.
pub fn linked_builds(app: &App) -> Vec<(String, Installed)> {
    let Some(steam) = locate(app) else {
        return Vec::new();
    };
    steam
        .linked(&app.store())
        .into_iter()
        .map(|(slot, build)| {
            (
                format!("Steam links to it ({})", slot.path.display()),
                build,
            )
        })
        .collect()
}

#[cfg_attr(feature = "json", derive(serde::Serialize))]
#[cfg_attr(not(feature = "json"), allow(dead_code))]
struct Row {
    runtime: String,
    client: bool,
    path: String,
    /// `linked`, `copy`, `changed` or `foreign`.
    state: &'static str,
    build: Option<String>,
    /// `changed`: the build Steam updated the linked one to.
    now: Option<String>,
    /// `foreign`: where the link leads.
    target: Option<String>,
}

fn row(app: &App, slot: &Slot) -> Result<Row, AppError> {
    let mut row = Row {
        runtime: slot.runtime.clone(),
        client: slot.client,
        path: slot.path.display().to_string(),
        state: "copy",
        build: None,
        now: None,
        target: None,
    };
    match slot.state(&app.store())? {
        State::Copy(build) => row.build = build.map(|b| b.to_string()),
        State::Linked(build) => {
            row.state = "linked";
            row.build = Some(build.build.to_string());
        }
        State::Changed { build, now } => {
            row.state = "changed";
            row.build = Some(build.build.to_string());
            row.now = now.map(|b| b.to_string());
        }
        State::Foreign(target) => {
            row.state = "foreign";
            row.target = Some(target.display().to_string());
        }
    }
    Ok(row)
}

/// `steam status`.
pub fn status(app: &mut App) -> Result<u8, AppError> {
    let steam = steam(app)?;
    let rows = steam
        .slots()
        .iter()
        .map(|slot| row(app, slot))
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&rows)?;
        return Ok(0);
    }
    app.out.info(format!("Steam: {}", steam.root().display()))?;
    if rows.is_empty() {
        app.out.info("Steam has no runtimes yet")?;
        return Ok(0);
    }
    let mut table = vec![vec![
        "RUNTIME".to_owned(),
        "BUILD".to_owned(),
        "STATE".to_owned(),
        "PATH".to_owned(),
    ]];
    for row in &rows {
        let state = match row.state {
            "linked" => app.out.green("linked"),
            "copy" => app.out.yellow("Steam's copy"),
            "changed" => app.out.red(&format!(
                "updated by Steam to {}",
                row.now.as_deref().unwrap_or("?")
            )),
            _ => app.out.red(&format!(
                "links to {}",
                row.target.as_deref().unwrap_or("?")
            )),
        };
        table.push(vec![
            row.runtime.clone(),
            row.build.clone().unwrap_or_else(|| "-".to_owned()),
            state,
            row.path.clone(),
        ]);
    }
    app.out.table(&table)?;
    if rows.iter().any(|r| matches!(r.state, "copy" | "changed")) {
        app.out
            .note("`sruman steam link` links them to sruman's builds")?;
    }
    Ok(0)
}

/// The slots `names` asks for (all without names).
fn select(steam: &Steam, names: &[String]) -> Result<Vec<Slot>, AppError> {
    let slots = steam.slots();
    if let Some(missing) = names
        .iter()
        .find(|name| !slots.iter().any(|s| &s.runtime == *name))
    {
        let mut has: Vec<&str> = slots.iter().map(|s| s.runtime.as_str()).collect();
        has.dedup();
        return Err(AppError::Invalid(format!(
            "Steam has no `{missing}`; it has: {}",
            if has.is_empty() {
                "none".to_owned()
            } else {
                has.join(", ")
            }
        )));
    }
    Ok(slots
        .into_iter()
        .filter(|slot| names.is_empty() || names.contains(&slot.runtime))
        .collect())
}

/// Runs `each` on every selected slot, printing failures and going on.
fn for_each(
    app: &mut App,
    names: &[String],
    mut each: impl FnMut(&mut App, &Steam, &Slot) -> Result<(), AppError>,
) -> Result<u8, AppError> {
    let steam = steam(app)?;
    if steam.running() {
        return Err(SteamError::Running.into());
    }
    let slots = select(&steam, names)?;
    if slots.is_empty() {
        app.out.info("Steam has no runtimes yet")?;
        return Ok(0);
    }
    let mut code = 0;
    for slot in &slots {
        if let Err(err) = each(app, &steam, slot) {
            code = code.max(err.exit_code());
            app.out.error(format!("{}: {err}", slot.path.display()))?;
        }
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}

/// `steam link [rt…]`.
pub fn link(app: &mut App, names: &[String]) -> Result<u8, AppError> {
    for_each(app, names, |app, steam, slot| {
        let linked = steam.link(&app.store(), app.catalog(), slot)?;
        let label = format!("{} {}", linked.build.name, linked.build.build);
        if let Some(old) = &linked.damaged {
            tracing::warn!(runtime = old.name, build = %old.build, "updated in place by Steam");
            app.out.warning(format!(
                "Steam updated {name} {build} in place, so its files no longer match it; \
                 `sruman steam clean` removes it (reinstall it with `sruman install {name}={build}` if you need it)",
                name = old.name,
                build = old.build
            ))?;
        }
        if !linked.changed {
            app.out.info(format!("{label}: linked already"))?;
            return Ok(());
        }
        tracing::info!(runtime = linked.build.name, build = %linked.build.build, path = %slot.path.display(), "linked for Steam");
        let how = if linked.imported {
            " (imported Steam's copy)"
        } else {
            ""
        };
        app.out.info(format!("linked {label} for Steam{how}"))?;
        Ok(())
    })
}

/// `steam unlink [rt…]`.
pub fn unlink(app: &mut App, names: &[String]) -> Result<u8, AppError> {
    for_each(app, names, |app, steam, slot| {
        if steam.unlink(&app.store(), slot)? {
            tracing::info!(runtime = slot.runtime, path = %slot.path.display(), "unlinked for Steam");
            app.out
                .info(format!("{}: Steam has its own copy again", slot.runtime))?;
        } else {
            app.out
                .info(format!("{}: Steam's own copy already", slot.runtime))?;
        }
        Ok(())
    })
}

/// `steam verify [rt…] [--full]`: the builds Steam links to, through
/// `verify`; a build Steam updated in place fails.
pub fn verify(app: &mut App, names: &[String], full: bool) -> Result<u8, AppError> {
    let steam = steam(app)?;
    let store = app.store();
    let mut targets = Vec::new();
    let mut bad = false;
    for slot in select(&steam, names)? {
        match slot.state(&store)? {
            State::Linked(build) => {
                if !targets.contains(&build.as_ref()) {
                    targets.push(build.as_ref());
                }
            }
            State::Changed { build, now } => {
                bad = true;
                app.out.error(format!(
                    "{} {}: updated by Steam to {}; `sruman steam link` takes the new build",
                    build.name,
                    build.build,
                    now.map_or_else(|| "?".to_owned(), |b| b.to_string())
                ))?;
            }
            State::Copy(_) => app.out.info(format!(
                "{}: Steam's copy, not sruman's; not checked",
                slot.runtime
            ))?,
            State::Foreign(target) => {
                bad = true;
                app.out.error(format!(
                    "{}: links to {}, not to a sruman build",
                    slot.path.display(),
                    target.display()
                ))?;
            }
        }
    }
    let code = if targets.is_empty() {
        0
    } else {
        super::runtimes::verify(app, &targets, full)?
    };
    Ok(code.max(u8::from(bad)))
}

/// `steam clean`: builds Steam updated in place that it no longer links
/// to (left by `steam link` moving the link to the new build).
pub fn clean(app: &mut App) -> Result<u8, AppError> {
    let store = app.store();
    let linked: Vec<Installed> = linked_builds(app).into_iter().map(|(_, b)| b).collect();
    let mut builds = Vec::new();
    for build in steam::rewritten(&store)? {
        if linked.contains(&build) {
            app.out.warning(format!(
                "{} {}: Steam updated it and still links to it; run `sruman steam link` first",
                build.name, build.build
            ))?;
        } else {
            builds.push(build);
        }
    }
    if builds.is_empty() {
        app.out.info("nothing to clean")?;
        return Ok(0);
    }
    let problems = super::runtimes::users(app, &store, &builds)?;
    if !problems.is_empty() {
        return Err(AppError::Invalid(format!(
            "{}\n(`sruman remove --force` removes them anyway)",
            problems.join("\n")
        )));
    }
    let labels: Vec<String> = builds
        .iter()
        .map(|b| format!("{}-{}", b.name, b.build))
        .collect();
    app.out.info(format!(
        "Steam updated these in place: {}",
        labels.join("  ")
    ))?;
    if !app.out.confirm("Remove them?", true)? {
        return Err(AppError::Cancelled);
    }
    let lock = store.lock()?;
    for build in &builds {
        store.remove(&lock, build, false)?;
        tracing::info!(runtime = build.name, build = %build.build, "removed after a Steam update");
        app.out
            .info(format!("removed {} {}", build.name, build.build))?;
    }
    Ok(0)
}
