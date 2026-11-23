//! Runtime commands: `sync`, `install`, `upgrade`, `outdated`, `remove`,
//! `search`, `info`, `runtimes`, `builds`, `files`, `verify`, `clean`,
//! `import`.

use std::path::{Path, PathBuf};

use libsruman::{
    model::{self, LoadedProfiles},
    mtree::{self, Depth},
    store::{
        Installed, Source, Store, StoreLock,
        import::{self, ImportMode},
    },
    version::{BuildId, RuntimeRef},
};
use serde::Serialize;

use super::date;
use crate::app::{App, AppError};

#[cfg(feature = "download")]
use libsruman::{
    fetch::{Fetch, UreqFetch},
    install::{SkipReason, Transaction, TxOptions, Upgrade, UpgradeOptions},
    progress::CancelFlag,
    sync::{self, RefreshOptions, RefreshOutcome},
};

// --- sync ---------------------------------------------------------------

/// `sync [--force]`.
#[cfg(feature = "download")]
pub fn sync(app: &mut App, force: bool) -> Result<u8, AppError> {
    refresh(app, &UreqFetch::new(), force)?;
    Ok(0)
}

/// Refreshes every catalog runtime's index; fails (already reported) when
/// one couldn't be refreshed.
#[cfg(feature = "download")]
fn refresh(app: &mut App, fetch: &dyn Fetch, force: bool) -> Result<(), AppError> {
    app.out.heading("Synchronizing the runtime index...")?;
    let sync_dir = app.sync_dir();
    let options = RefreshOptions {
        force,
        only: Vec::new(),
    };
    let mut failed = false;
    for (name, outcome) in sync::refresh(fetch, app.catalog(), &sync_dir, &options) {
        match outcome {
            RefreshOutcome::UpToDate => app.out.info(format!(" {name} is up to date"))?,
            RefreshOutcome::Updated { .. } => {
                let channel = app.settings.channels.get(&name);
                let build = sync_dir
                    .load(&name)
                    .and_then(|index| index.channels.get(&channel).cloned());
                tracing::info!(runtime = name, "index updated");
                match build {
                    Some(build) => app.out.info(format!(" {name} {build} ({channel})"))?,
                    None => app.out.info(format!(" {name}: no {channel} build"))?,
                }
            }
            RefreshOutcome::Failed(err) => {
                failed = true;
                tracing::error!(runtime = name, "sync failed: {err}");
                app.out
                    .error(format!("failed to synchronize {name}: {err}"))?;
            }
        }
    }
    if failed {
        Err(AppError::Reported(1))
    } else {
        Ok(())
    }
}

// --- install / upgrade --------------------------------------------------

#[cfg(feature = "download")]
fn tx_options(app: &App, download_only: bool) -> Result<TxOptions, AppError> {
    Ok(TxOptions {
        channel: app.settings.channels.default,
        channels: app.settings.channels.per_runtime.clone(),
        download_only,
        ..TxOptions::new()?
    })
}

/// `install <rt>[=build]…`.
#[cfg(feature = "download")]
pub fn install(
    app: &mut App,
    targets: &[RuntimeRef],
    needed: bool,
    download_only: bool,
) -> Result<u8, AppError> {
    let fetch = UreqFetch::new();
    install_with(app, &fetch, targets, needed, download_only, true)
}

/// `install` with a given client; also D12's install before a launch,
/// which asked already (`ask` false).
#[cfg(feature = "download")]
pub fn install_with(
    app: &mut App,
    fetch: &dyn Fetch,
    targets: &[RuntimeRef],
    needed: bool,
    download_only: bool,
    ask: bool,
) -> Result<u8, AppError> {
    let store = app.store();
    let options = tx_options(app, download_only)?;
    app.out.info("resolving dependencies...")?;
    let tx = Transaction::install(
        fetch,
        &app.sync_dir(),
        &store,
        app.catalog(),
        targets,
        &options,
    )?;
    for skipped in &tx.skipped {
        if skipped.reason == SkipReason::Installed && !needed {
            let build = skipped
                .build
                .as_ref()
                .map_or_else(String::new, |b| format!("-{b}"));
            app.out
                .warning(format!("{}{build} is up to date -- skipping", skipped.name))?;
        }
    }
    let verb = if download_only {
        "download"
    } else {
        "installation"
    };
    commit(app, fetch, &store, &tx, verb, ask)
}

/// `upgrade [rt…]`.
#[cfg(feature = "download")]
pub fn upgrade(
    app: &mut App,
    targets: &[String],
    sync: bool,
    allow_downgrade: bool,
    download_only: bool,
) -> Result<u8, AppError> {
    let fetch = UreqFetch::new();
    // A runtime that failed to refresh still upgrades from its old index;
    // the failure shows in the exit code.
    let mut sync_failed = false;
    if sync && let Err(err) = refresh(app, &fetch, false) {
        if !err.is_reported() {
            return Err(err);
        }
        sync_failed = true;
    }
    app.out.heading("Starting runtime upgrade...")?;
    let store = app.store();
    let tx = upgrade_tx(app, &fetch, &store, targets, allow_downgrade, download_only)?;
    for skipped in &tx.skipped {
        if skipped.reason == SkipReason::NotSynced {
            app.out.warning(format!(
                "{}: not in the sync index; run `sruman sync`",
                skipped.name
            ))?;
        }
    }
    let verb = if download_only { "download" } else { "upgrade" };
    let code = commit(app, &fetch, &store, &tx, verb, true)?;
    Ok(if sync_failed { code.max(1) } else { code })
}

#[cfg(feature = "download")]
fn upgrade_tx(
    app: &App,
    fetch: &dyn Fetch,
    store: &Store,
    targets: &[String],
    allow_downgrade: bool,
    download_only: bool,
) -> Result<Transaction, AppError> {
    let which = if targets.is_empty() {
        Upgrade::All
    } else {
        Upgrade::Named(targets.to_vec())
    };
    Ok(Transaction::upgrade(
        fetch,
        &app.sync_dir(),
        store,
        app.catalog(),
        &which,
        &tx_options(app, download_only)?,
        &UpgradeOptions { allow_downgrade },
    )?)
}

/// Prints the transaction, asks, and commits it under the store lock.
#[cfg(feature = "download")]
fn commit(
    app: &mut App,
    fetch: &dyn Fetch,
    store: &Store,
    tx: &Transaction,
    verb: &str,
    ask: bool,
) -> Result<u8, AppError> {
    if tx.is_empty() {
        app.out.info(" there is nothing to do")?;
        return Ok(0);
    }
    let pkg_cache = &app.settings.dirs.app.pkg_cache;
    let summary = tx.summary(&app.sync_dir(), store, pkg_cache);
    for (name, diff) in &summary.changes {
        if diff.is_empty() {
            continue;
        }
        app.out.heading(format!("Package changes in {name}:"))?;
        for line in diff.summary() {
            app.out.info(format!("  {line}"))?;
        }
    }
    for switch in &tx.switches {
        let from = switch
            .from
            .as_ref()
            .map_or_else(|| "none".to_owned(), ToString::to_string);
        app.out.info(format!(
            "{}: current {from} -> {} (installed already)",
            switch.name, switch.to
        ))?;
    }
    if !tx.items.is_empty() {
        let labels: Vec<String> = tx
            .items
            .iter()
            .map(|item| format!("{}-{}", item.name, item.build))
            .collect();
        app.out.info("")?;
        let title = app.out.bold(&format!("Runtimes ({})", tx.items.len()));
        app.out.info(format!("{title}  {}", labels.join("  ")))?;
        app.out.info("")?;
        let mut rows = vec![vec![
            app.out.bold("Total Download Size:"),
            crate::output::size(summary.download_size),
        ]];
        if summary.installed_size_kib > 0 {
            rows.push(vec![
                app.out.bold("Total Installed Size:"),
                crate::output::size(summary.installed_size_kib * 1024),
            ]);
        }
        if !app.out.quiet {
            app.out.table(&rows)?;
        }
        app.out.info("")?;
    }
    if ask && !app.out.confirm(&format!("Proceed with {verb}?"), true)? {
        return Err(AppError::Cancelled);
    }
    let lock = store.lock()?;
    let report = {
        let mut bar = app.out.progress();
        tx.commit(fetch, store, &lock, pkg_cache, &mut bar, &CancelFlag::new())
    };
    drop(lock);
    for switch in &report.switched {
        tracing::info!(runtime = switch.name, build = %switch.to, "current switched");
    }
    for installed in &report.installed {
        tracing::info!(runtime = installed.name, build = %installed.build, "installed");
    }
    for path in &report.downloaded {
        app.out.info(format!("downloaded {}", path.display()))?;
    }
    match report.failed {
        None => Ok(0),
        Some((label, err)) => {
            tracing::error!("{label}: {err}");
            if !report.not_attempted.is_empty() {
                app.out.warning(format!(
                    "not attempted: {}",
                    report.not_attempted.join(", ")
                ))?;
            }
            let err = AppError::from(err);
            Err(match err {
                AppError::Cancelled => err,
                AppError::Invalid(message) => AppError::Invalid(format!("{label}: {message}")),
                AppError::Busy(message) => AppError::Busy(format!("{label}: {message}")),
                other => AppError::Failed(format!("{label}: {other}")),
            })
        }
    }
}

/// One row of `outdated`.
#[cfg(feature = "download")]
#[derive(Debug, Serialize)]
struct Outdated {
    name: String,
    current: Option<BuildId>,
    new: BuildId,
    /// The new build is installed already; only `current` moves.
    installed: bool,
}

/// `outdated [rt…]`.
#[cfg(feature = "download")]
pub fn outdated(app: &mut App, targets: &[String]) -> Result<u8, AppError> {
    let store = app.store();
    let tx = upgrade_tx(app, &UreqFetch::new(), &store, targets, false, false)?;
    let mut rows: Vec<Outdated> = tx
        .switches
        .iter()
        .map(|switch| Outdated {
            name: switch.name.clone(),
            current: switch.from.clone(),
            new: switch.to.clone(),
            installed: true,
        })
        .collect();
    rows.extend(tx.items.iter().map(|item| Outdated {
        name: item.name.clone(),
        current: item.current.clone(),
        new: item.build.clone(),
        installed: false,
    }));
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&rows)?;
        return Ok(0);
    }
    for row in &rows {
        let current = row
            .current
            .as_ref()
            .map_or_else(|| "none".to_owned(), ToString::to_string);
        let line = if app.out.quiet {
            row.name.clone()
        } else {
            format!(
                "{} {current} -> {}",
                app.out.bold(&row.name),
                app.out.green(row.new.as_str())
            )
        };
        app.out.line(line)?;
    }
    Ok(0)
}

// --- remove ---------------------------------------------------------------

/// `remove <rt>[=build]…`.
pub fn remove(app: &mut App, targets: &[RuntimeRef], force: bool) -> Result<u8, AppError> {
    let store = app.store();
    let mut builds: Vec<Installed> = Vec::new();
    for target in targets {
        let found = match &target.build {
            Some(_) => vec![store.resolve(target)?],
            None => {
                let all = store.installed_of(&target.name)?;
                if all.is_empty() {
                    // The library's message names the fix.
                    store.resolve(target)?;
                }
                all
            }
        };
        for build in found {
            if !builds.contains(&build) {
                builds.push(build);
            }
        }
    }
    // Steam's builds are `sruman steam`'s business, `--force` or not.
    let steam: Vec<String> = super::steam::linked_builds(app)
        .into_iter()
        .filter(|(_, linked)| builds.contains(linked))
        .map(|(why, build)| {
            format!(
                "{} {}: {why}; `sruman steam unlink {}` first",
                build.name, build.build, build.name
            )
        })
        .collect();
    if !steam.is_empty() {
        return Err(AppError::Invalid(steam.join("\n")));
    }
    if !force {
        let problems = users(app, &store, &builds)?;
        if !problems.is_empty() {
            return Err(AppError::Invalid(format!(
                "{}\n(`--force` skips this check; a build in use is never removed)",
                problems.join("\n")
            )));
        }
    }
    let labels: Vec<String> = builds
        .iter()
        .map(|b| format!("{}-{}", b.name, b.build))
        .collect();
    app.out.info("")?;
    let title = app.out.bold(&format!("Runtimes ({})", builds.len()));
    app.out.info(format!("{title}  {}", labels.join("  ")))?;
    app.out.info("")?;
    if !app
        .out
        .confirm("Do you want to remove these runtimes?", true)?
    {
        return Err(AppError::Cancelled);
    }
    let lock = store.lock()?;
    let mut code = 0;
    for build in &builds {
        match store.remove(&lock, build, false) {
            Ok(()) => {
                tracing::info!(runtime = build.name, build = %build.build, "removed");
                let what = if build.is_linked() {
                    "unregistered"
                } else {
                    "removed"
                };
                app.out
                    .info(format!("{what} {} {}", build.name, build.build))?;
            }
            Err(err) => {
                let err = AppError::from(err);
                code = code.max(err.exit_code());
                app.out.error(err.to_string())?;
            }
        }
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}

/// Why each of `builds` must stay: profiles that use it, and installed
/// runtimes that run inside it.
pub(super) fn users(
    app: &App,
    store: &Store,
    builds: &[Installed],
) -> Result<Vec<String>, AppError> {
    let profiles = app.profiles();
    let mut problems = Vec::new();
    for build in builds {
        let left = store
            .installed_of(&build.name)?
            .into_iter()
            .filter(|other| !builds.contains(other))
            .count();
        for user in profile_users(&profiles, &build.name, &build.build, left == 0) {
            problems.push(format!(
                "{} {} is used by profile `{user}`",
                build.name, build.build
            ));
        }
        if left == 0 {
            for dependent in app.catalog().dependents(&build.name) {
                let removed_too = builds.iter().any(|b| b.name == dependent.name)
                    && store
                        .installed_of(&dependent.name)?
                        .iter()
                        .all(|b| builds.contains(b));
                if !removed_too && !store.installed_of(&dependent.name)?.is_empty() {
                    problems.push(format!(
                        "{} runs inside {}, which would be gone",
                        dependent.name, build.name
                    ));
                }
            }
        }
    }
    Ok(problems)
}

/// Profile files whose runtime is `name`: pinned to `build`, or unpinned
/// when `last` (no build of `name` would be left).
fn profile_users(
    profiles: &LoadedProfiles,
    name: &str,
    build: &BuildId,
    last: bool,
) -> Vec<String> {
    profiles
        .profiles
        .values()
        .filter(|profile| {
            model::slr(profile).is_ok_and(|(_, runtime)| {
                runtime.name == name
                    && match &runtime.build {
                        Some(pinned) => pinned == build,
                        None => last,
                    }
            })
        })
        .map(|profile| profile.name.clone())
        .collect()
}

// --- search ---------------------------------------------------------------

/// `search [term…] [--packages]`: case-insensitive substrings.
#[cfg(feature = "download")]
pub fn search(app: &mut App, terms: &[String], packages: bool) -> Result<u8, AppError> {
    let sync_dir = app.sync_dir();
    let store = app.store();
    let terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let matches = |text: &str| {
        let text = text.to_lowercase();
        terms.is_empty() || terms.iter().any(|term| text.contains(term))
    };
    let mut found = 0;
    let defs: Vec<_> = app.catalog().iter().cloned().collect();
    for def in defs {
        let channel = app.settings.channels.get(&def.name);
        let index = sync_dir.load(&def.name);
        let build = index
            .as_ref()
            .and_then(|i| i.channels.get(&channel).cloned());
        if packages {
            let Some(build) = &build else { continue };
            let Some(manifest) = sync_dir.manifest(&def.name, build) else {
                continue;
            };
            for package in manifest.iter().filter(|p| matches(&p.name)) {
                found += 1;
                let arch = package
                    .arch
                    .as_deref()
                    .map_or_else(String::new, |a| format!(":{a}"));
                let line = if app.out.quiet {
                    package.name.clone()
                } else {
                    format!(
                        "{}/{}{arch} {}",
                        def.name,
                        app.out.bold(&package.name),
                        app.out.green(&package.version)
                    )
                };
                app.out.line(line)?;
            }
            continue;
        }
        if !(matches(&def.name) || matches(&def.description)) {
            continue;
        }
        found += 1;
        if app.out.quiet {
            app.out.line(&def.name)?;
            continue;
        }
        let version = build
            .as_ref()
            .map_or_else(|| "(run `sruman sync`)".to_owned(), ToString::to_string);
        let installed = match store.current(&def.name) {
            Some(current) if Some(&current) == build.as_ref() => " [installed]".to_owned(),
            Some(current) => format!(" [installed: {current}]"),
            None => String::new(),
        };
        let line = format!(
            "{} {}{}",
            app.out.bold(&def.name),
            app.out.green(&version),
            app.out.yellow(&installed)
        );
        app.out.line(line)?;
        app.out.line(format!("    {}", def.description))?;
    }
    Ok(if found == 0 { 1 } else { 0 })
}

// --- info -------------------------------------------------------------------

/// `info` as JSON.
#[derive(Debug, Serialize)]
struct Info {
    name: String,
    build: BuildId,
    description: String,
    channel: String,
    installed: bool,
    current: bool,
    source: Option<Source>,
    installed_at: Option<u64>,
    dir: Option<PathBuf>,
    depends: Vec<String>,
    required_by: Vec<String>,
    in_use: bool,
    download_size: Option<u64>,
    os: Option<String>,
    package_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packages: Option<Vec<(String, String)>>,
}

/// `info <rt>[=build]…`: the installed build, else the sync index.
pub fn info(app: &mut App, targets: &[RuntimeRef], packages: bool) -> Result<u8, AppError> {
    let store = app.store();
    let profiles = app.profiles();
    let mut all = Vec::new();
    for target in targets {
        let Some(def) = app.catalog().get(&target.name).cloned() else {
            return Err(AppError::Invalid(format!(
                "unknown runtime `{}`; known: {}",
                target.name,
                app.catalog().names().collect::<Vec<_>>().join(", ")
            )));
        };
        let channel = app.settings.channels.get(&def.name);
        let installed = store.resolve(target).ok();
        let (build, os, download_size, manifest) = match &installed {
            Some(build) => (
                build.build.clone(),
                os_release(&build.dir),
                None,
                store.manifest(build),
            ),
            None => remote_info(app, target)?,
        };
        let required_by: Vec<String> = profile_users(&profiles, &def.name, &build, true)
            .into_iter()
            .map(|p| format!("profile {p}"))
            .chain(
                app.catalog()
                    .dependents(&def.name)
                    .into_iter()
                    .map(|d| d.name.clone()),
            )
            .collect();
        all.push(Info {
            name: def.name.clone(),
            build,
            description: def.description.clone(),
            channel: channel.to_string(),
            installed: installed.is_some(),
            current: installed.as_ref().is_some_and(|b| b.is_current),
            source: installed
                .as_ref()
                .and_then(|b| b.meta.as_ref())
                .map(|m| m.source.clone()),
            installed_at: installed
                .as_ref()
                .and_then(|b| b.meta.as_ref())
                .map(|m| m.installed_at),
            dir: installed.as_ref().map(|b| b.dir.clone()),
            depends: def.depends.clone(),
            required_by,
            in_use: installed.as_ref().is_some_and(|b| store.in_use(b)),
            download_size,
            os,
            package_count: manifest.as_ref().map(|m| m.len()),
            packages: packages.then(|| {
                manifest
                    .iter()
                    .flat_map(|m| m.iter())
                    .map(|p| (p.name.clone(), p.version.clone()))
                    .collect()
            }),
        });
    }
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&all)?;
        return Ok(0);
    }
    for (n, info) in all.iter().enumerate() {
        if n > 0 {
            app.out.line("")?;
        }
        print_info(app, info)?;
    }
    Ok(0)
}

fn print_info(app: &mut App, info: &Info) -> Result<(), AppError> {
    let none = || "None".to_owned();
    let mut rows: Vec<(&str, String)> = vec![
        ("Name", info.name.clone()),
        ("Build", info.build.to_string()),
        ("Description", info.description.clone()),
        ("Channel", info.channel.clone()),
        ("OS", info.os.clone().unwrap_or_else(none)),
        (
            "Depends On",
            if info.depends.is_empty() {
                none()
            } else {
                info.depends.join("  ")
            },
        ),
        (
            "Required By",
            if info.required_by.is_empty() {
                none()
            } else {
                info.required_by.join("  ")
            },
        ),
    ];
    if let Some(count) = info.package_count {
        rows.push(("Packages", count.to_string()));
    }
    if let Some(size) = info.download_size {
        rows.push(("Download Size", crate::output::size(size)));
    }
    if info.installed {
        rows.push(("Current", yes_no(info.current)));
        rows.push(("In Use", yes_no(info.in_use)));
        if let Some(source) = &info.source {
            rows.push(("Source", describe_source(source)));
        }
        if let Some(at) = info.installed_at {
            rows.push(("Install Date", date(at)));
        }
        if let Some(dir) = &info.dir {
            rows.push(("Directory", dir.display().to_string()));
        }
    } else {
        rows.push(("Installed", "No".to_owned()));
    }
    let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in rows {
        let key = app.out.bold(&format!("{key:<width$}"));
        app.out.line(format!("{key} : {value}"))?;
    }
    if let Some(packages) = &info.packages {
        app.out.line("")?;
        for (name, version) in packages {
            app.out.line(format!("{name} {version}"))?;
        }
    }
    Ok(())
}

fn yes_no(value: bool) -> String {
    if value { "Yes" } else { "No" }.to_owned()
}

fn describe_source(source: &Source) -> String {
    match source {
        Source::Download { url, .. } => url.clone(),
        Source::Import { from } => format!("imported from {}", from.display()),
        Source::Link { target } => format!("linked to {}", target.display()),
    }
}

/// `PRETTY_NAME` from a build's `os-release` (container runtimes keep it in
/// the platform's `files/lib/os-release`).
fn os_release(build: &Path) -> Option<String> {
    let platform = std::fs::read_dir(build)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("_platform_"))
        })?;
    ["files/lib/os-release", "files/etc/os-release"]
        .iter()
        .find_map(|file| std::fs::read_to_string(platform.join(file)).ok())
        .and_then(|text| pretty_name(&text))
}

fn pretty_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|v| v.trim_matches('"').to_owned())
    })
}

type Remote = (
    BuildId,
    Option<String>,
    Option<u64>,
    Option<libsruman::manifest::Manifest>,
);

/// `info` on a runtime that isn't installed: the sync index.
#[cfg(feature = "download")]
fn remote_info(app: &App, target: &RuntimeRef) -> Result<Remote, AppError> {
    let sync_dir = app.sync_dir();
    let not_synced = || {
        AppError::Invalid(format!(
            "`{target}` is neither installed nor in the sync index; run `sruman sync`"
        ))
    };
    let index = sync_dir.load(&target.name).ok_or_else(not_synced)?;
    let build = match &target.build {
        Some(build) => build.clone(),
        None => {
            let channel = app.settings.channels.get(&target.name);
            index
                .channels
                .get(&channel)
                .cloned()
                .ok_or_else(not_synced)?
        }
    };
    let info = index.builds.get(&build).ok_or_else(not_synced)?;
    let arch = libsruman::catalog::Arch::current();
    let size = arch
        .and_then(|a| info.archives.get(&a))
        .and_then(|a| a.size);
    let os = info.os_release.get("PRETTY_NAME").cloned();
    let manifest = sync_dir.manifest(&target.name, &build);
    Ok((build, os, size, manifest))
}

#[cfg(not(feature = "download"))]
fn remote_info(_: &App, target: &RuntimeRef) -> Result<Remote, AppError> {
    Err(AppError::Invalid(format!(
        "`{target}` is not installed; this sruman is built without downloads, so import it with `sruman import <dir>`"
    )))
}

// --- runtimes / builds / files ------------------------------------------------

#[cfg(feature = "json")]
#[derive(Debug, Serialize)]
struct InstalledRow {
    name: String,
    build: BuildId,
    current: bool,
    linked: bool,
    dir: PathBuf,
    source: Option<Source>,
}

/// `runtimes [-q]`.
pub fn runtimes(app: &mut App, quiet: bool) -> Result<u8, AppError> {
    let installed = app.store().installed()?;
    #[cfg(feature = "json")]
    if app.out.json {
        let rows: Vec<InstalledRow> = installed
            .iter()
            .map(|b| InstalledRow {
                name: b.name.clone(),
                build: b.build.clone(),
                current: b.is_current,
                linked: b.is_linked(),
                dir: b.dir.clone(),
                source: b.meta.as_ref().map(|m| m.source.clone()),
            })
            .collect();
        app.out.json(&rows)?;
        return Ok(0);
    }
    if quiet {
        let mut names: Vec<&str> = installed.iter().map(|b| b.name.as_str()).collect();
        names.dedup();
        for name in names {
            app.out.line(name)?;
        }
        return Ok(0);
    }
    for build in &installed {
        let several = installed.iter().filter(|b| b.name == build.name).count() > 1;
        let mut notes = Vec::new();
        if several && build.is_current {
            notes.push("current");
        }
        if build.is_linked() {
            notes.push("linked");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", notes.join(", "))
        };
        let line = format!(
            "{} {}{}",
            app.out.bold(&build.name),
            app.out.green(build.build.as_str()),
            notes
        );
        app.out.line(line)?;
    }
    Ok(0)
}

#[cfg(feature = "download")]
#[derive(Debug, Serialize)]
struct BuildRow {
    build: BuildId,
    installed: bool,
    current: bool,
    channels: Vec<String>,
}

/// `builds <rt>`: the server's builds, oldest first.
#[cfg(feature = "download")]
pub fn builds(app: &mut App, runtime: &str) -> Result<u8, AppError> {
    let def = app
        .catalog()
        .get(runtime)
        .cloned()
        .ok_or_else(|| AppError::Invalid(format!("unknown runtime `{runtime}`")))?;
    let store = app.store();
    let index = app.sync_dir().load(runtime);
    // The major version of the builds this runtime publishes now.
    let major = index
        .as_ref()
        .and_then(|i| i.channels.values().max().cloned())
        .or_else(|| store.current(runtime))
        .and_then(|b| b.parts().first().copied());
    let ids = sync::list_builds(&UreqFetch::new(), &def, major)?;
    let installed = store.installed_of(runtime)?;
    let rows: Vec<BuildRow> = ids
        .into_iter()
        .map(|build| BuildRow {
            installed: installed.iter().any(|b| b.build == build),
            current: installed.iter().any(|b| b.build == build && b.is_current),
            channels: index
                .iter()
                .flat_map(|i| i.channels.iter())
                .filter(|(_, b)| **b == build)
                .map(|(c, _)| c.to_string())
                .collect(),
            build,
        })
        .collect();
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&rows)?;
        return Ok(0);
    }
    for row in &rows {
        let mut notes: Vec<String> = row.channels.clone();
        if row.current {
            notes.push("current".into());
        } else if row.installed {
            notes.push("installed".into());
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            app.out.yellow(&format!(" [{}]", notes.join(", ")))
        };
        app.out.line(format!("{}{notes}", row.build))?;
    }
    Ok(0)
}

/// `files <rt>[=build]`: every path the build's file lists name.
pub fn files(app: &mut App, runtime: &RuntimeRef) -> Result<u8, AppError> {
    let build = app.store().resolve(runtime)?;
    let top = build.dir.join("mtree.txt.gz");
    if !top.is_file() {
        return Err(AppError::Failed(format!(
            "{} {} has no file list (mtree.txt.gz)",
            build.name, build.build
        )));
    }
    let failed = |e: mtree::MtreeError| AppError::Failed(e.to_string());
    let mut paths: Vec<PathBuf> = mtree::read_gz(&top)
        .map_err(failed)?
        .into_iter()
        .map(|entry| build.dir.join(entry.path))
        .collect();
    for platform in std::fs::read_dir(&build.dir)
        .map_err(|e| AppError::Failed(format!("{}: {e}", build.dir.display())))?
        .flatten()
        .map(|e| e.path())
    {
        let usr = platform.join("usr-mtree.txt.gz");
        if usr.is_file() {
            let files = platform.join("files");
            paths.extend(
                mtree::read_gz(&usr)
                    .map_err(failed)?
                    .into_iter()
                    .map(|entry| files.join(entry.path)),
            );
        }
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let path = path.display().to_string();
        app.out.line(path.trim_end_matches("/."))?;
    }
    Ok(0)
}

// --- verify -------------------------------------------------------------------

/// `verify [rt[=build]…] [--full]`: every installed build without targets.
pub fn verify(app: &mut App, targets: &[RuntimeRef], full: bool) -> Result<u8, AppError> {
    let store = app.store();
    let builds = if targets.is_empty() {
        store.installed()?
    } else {
        targets
            .iter()
            .map(|t| store.resolve(t))
            .collect::<Result<Vec<_>, _>>()?
    };
    if builds.is_empty() {
        app.out.info("no runtimes installed")?;
        return Ok(0);
    }
    let depth = if full {
        Depth::Content
    } else {
        Depth::Presence
    };
    let mut bad = false;
    for build in &builds {
        let label = format!("{} {}", build.name, build.build);
        match store.verify(build, depth) {
            Ok(None) => app
                .out
                .warning(format!("{label}: no file list; not checked"))?,
            Ok(Some(report)) if report.problems.is_empty() => {
                let ok = app.out.green("ok");
                app.out
                    .info(format!("{label}: {} files, {ok}", report.checked))?;
            }
            Ok(Some(report)) => {
                bad = true;
                for problem in &report.problems {
                    app.out.line(format!("{label}: {problem}"))?;
                }
                app.out.error(format!(
                    "{label}: {} of {} files have problems",
                    report.problems.len(),
                    report.checked
                ))?;
            }
            Err(err) => {
                bad = true;
                app.out.error(format!("{label}: {err}"))?;
            }
        }
    }
    Ok(u8::from(bad))
}

// --- clean ----------------------------------------------------------------------

/// `clean [--all]`: old builds beyond `keep_versions`, and cached archives
/// of builds that aren't installed (all of them with `--all`).
#[cfg(feature = "download")]
pub fn clean(app: &mut App, all: bool) -> Result<u8, AppError> {
    use libsruman::store::{CleanPolicy, KeptBecause};

    let question = if all {
        "Remove old runtime builds and ALL cached archives?"
    } else {
        "Remove old runtime builds and the archives of builds not installed?"
    };
    if !app.out.confirm(question, true)? {
        return Err(AppError::Cancelled);
    }
    let store = app.store();
    let lock = store.lock()?;
    let protect = pinned(&app.profiles());
    let steam = super::steam::linked_builds(app)
        .iter()
        .map(|(_, build)| build.as_ref())
        .collect();
    let report = store.clean(
        &lock,
        &CleanPolicy {
            keep: app.settings.keep_versions,
            protect,
            steam,
        },
    )?;
    for build in &report.removed {
        tracing::info!(runtime = build.name, build = %build.build, "cleaned");
        app.out
            .info(format!("removed {} {}", build.name, build.build))?;
    }
    for (build, why) in &report.kept {
        let why = match why {
            KeptBecause::InUse => "in use",
            KeptBecause::Linked => "linked",
            KeptBecause::Steam => "Steam links to it; see `sruman steam`",
        };
        app.out
            .info(format!("kept {} {} ({why})", build.name, build.build))?;
    }
    let (count, bytes) = clean_cache(&store, &lock, &app.settings.dirs.app.pkg_cache, all)?;
    app.out.info(format!(
        "removed {count} cached archive{} ({})",
        if count == 1 { "" } else { "s" },
        crate::output::size(bytes)
    ))?;
    Ok(0)
}

/// Builds that profiles pin, which `clean` keeps.
#[cfg(feature = "download")]
fn pinned(profiles: &LoadedProfiles) -> Vec<RuntimeRef> {
    profiles
        .profiles
        .values()
        .filter_map(|p| model::slr(p).ok().map(|(_, runtime)| runtime))
        .filter(RuntimeRef::is_pinned)
        .collect()
}

/// Deletes cached archives (`<name>-<build>-<file>`, and `.part`s): those
/// of builds that aren't installed, or all. Returns the count and bytes.
#[cfg(feature = "download")]
fn clean_cache(
    store: &Store,
    _lock: &StoreLock,
    cache: &Path,
    all: bool,
) -> Result<(usize, u64), AppError> {
    let installed = store.installed()?;
    let entries = match std::fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(AppError::Failed(format!("{}: {e}", cache.display()))),
    };
    let (mut count, mut bytes) = (0, 0);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let keep = !all
            && installed
                .iter()
                .any(|b| name.starts_with(&format!("{}-{}-", b.name, b.build)));
        if keep || !entry.path().is_file() {
            continue;
        }
        let size = entry.metadata().map_or(0, |m| m.len());
        std::fs::remove_file(entry.path())
            .map_err(|e| AppError::Failed(format!("{}: {e}", entry.path().display())))?;
        count += 1;
        bytes += size;
    }
    Ok((count, bytes))
}

// --- import ---------------------------------------------------------------------

/// `import <archive|dir>… [--link]`.
pub fn import(app: &mut App, sources: &[PathBuf], link: bool) -> Result<u8, AppError> {
    let store = app.store();
    let lock = store.lock()?;
    let mode = if link {
        ImportMode::Link
    } else {
        ImportMode::Copy
    };
    let mut code = 0;
    for source in sources {
        let result = if source.is_file() {
            if link {
                Err(AppError::Invalid(format!(
                    "{}: `--link` takes a directory, not an archive",
                    source.display()
                )))
            } else {
                import_archive(app, &store, &lock, source)
            }
        } else {
            import::import(&store, &lock, app.catalog(), source, None, mode).map_err(Into::into)
        };
        match result {
            Ok(installed) => {
                tracing::info!(runtime = installed.name, build = %installed.build, from = %source.display(), "imported");
                let how = if link { "linked" } else { "imported" };
                let current = if installed.is_current {
                    " (current)"
                } else {
                    ""
                };
                app.out.info(format!(
                    "{how} {} {}{current}",
                    installed.name, installed.build
                ))?;
            }
            Err(err) => {
                code = code.max(err.exit_code());
                // Most errors name the path already.
                let (shown, message) = (source.display().to_string(), err.to_string());
                if message.starts_with(&shown) {
                    app.out.error(message)?;
                } else {
                    app.out.error(format!("{shown}: {message}"))?;
                }
            }
        }
    }
    if code == 0 {
        Ok(0)
    } else {
        Err(AppError::Reported(code))
    }
}

/// Unpacks a runtime archive (`SteamLinuxRuntime_sniper.tar.xz`) in the
/// store's staging area and commits it.
#[cfg(feature = "download")]
fn import_archive(
    app: &mut App,
    store: &Store,
    lock: &StoreLock,
    archive: &Path,
) -> Result<Installed, AppError> {
    use libsruman::{
        install::extract::{Limits, extract_tar_xz},
        store::{BuildMeta, MakeCurrent},
    };

    let staging = store.staging_dir(lock, "import-archive")?;
    let tree = staging.join("build");
    std::fs::create_dir(&tree).map_err(|e| AppError::Failed(format!("{}: {e}", tree.display())))?;
    {
        let mut bar = app.out.progress();
        libsruman::progress::Progress::begin(&mut bar, &archive.display().to_string());
        let size = std::fs::metadata(archive).map_or(0, |m| m.len());
        libsruman::progress::Progress::phase(
            &mut bar,
            libsruman::progress::Phase::Extract,
            Some(size),
        );
        // The build's manifest is inside the archive, so the cap follows
        // the archive's size.
        let limits = Limits::for_build(None, size);
        let result = extract_tar_xz(archive, &tree, limits, &mut bar, &CancelFlag::new());
        libsruman::progress::Progress::end(&mut bar, result.is_ok());
        result.map_err(|e| AppError::Failed(format!("{}: {e}", archive.display())))?;
    }
    let found = import::detect(&tree, app.catalog())?;
    let from = std::fs::canonicalize(archive).unwrap_or_else(|_| archive.to_path_buf());
    let meta = BuildMeta::new(&found.name, &found.build, Source::Import { from });
    Ok(store.commit(lock, &tree, meta, MakeCurrent::IfNewer)?)
}

#[cfg(not(feature = "download"))]
fn import_archive(
    _: &mut App,
    _: &Store,
    _: &StoreLock,
    archive: &Path,
) -> Result<Installed, AppError> {
    Err(AppError::Invalid(format!(
        "{}: importing archives needs sruman built with `download`; unpack it and import the directory",
        archive.display()
    )))
}
