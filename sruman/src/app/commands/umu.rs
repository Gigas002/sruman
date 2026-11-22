//! `provide-umu` (D15): hand a runtime to umu in its own folder layout.

use libsruman::{
    umu::{self, Folders, UmuError},
    version::RuntimeRef,
};

use super::super::{App, AppError, supervisor};

impl From<UmuError> for AppError {
    fn from(err: UmuError) -> Self {
        match err {
            UmuError::Store(err) => err.into(),
            err => Self::Failed(err.to_string()),
        }
    }
}

/// What lootrs reads with `--json`: the `UMU_FOLDERS_PATH`, umu's folder
/// name, and the build's hold file to `flock` shared for the run.
#[cfg_attr(feature = "json", derive(serde::Serialize))]
#[cfg_attr(not(feature = "json"), allow(dead_code))]
struct Provided {
    folders: String,
    variant: &'static str,
    runtime: String,
    build: String,
    dir: String,
    hold: String,
    /// Left on an older build a running game uses.
    kept: bool,
}

pub fn provide(
    app: &mut App,
    runtime: Option<&RuntimeRef>,
    app_id: Option<u32>,
) -> Result<u8, AppError> {
    let def = match (runtime, app_id) {
        (Some(runtime), _) => app
            .catalog()
            .get(&runtime.name)
            .ok_or_else(|| AppError::Invalid(format!("unknown runtime `{}`", runtime.name)))?,
        (None, Some(id)) => app.catalog().by_app_id(id).ok_or_else(|| {
            AppError::Invalid(format!(
                "no runtime has Steam app id {id}; known: soldier 1391110, sniper 1628350, steamrt4 4183110"
            ))
        })?,
        (None, None) => unreachable!("clap requires a runtime or --appid"),
    };
    let Some(variant) = def.app_id.and_then(umu::variant) else {
        return Err(AppError::Invalid(format!(
            "umu has no folder for {}; it runs soldier, sniper and steamrt4",
            def.name
        )));
    };
    let wanted = runtime.cloned().unwrap_or_else(|| RuntimeRef {
        name: def.name.clone(),
        build: None,
    });
    let store = app.store();
    if store.resolve(&wanted).is_err() {
        supervisor::install_missing(app, std::slice::from_ref(&wanted), "umu")?;
    }
    let build = store.resolve(&wanted)?;
    let folders = Folders::new(&app.settings.dirs.app.umu_folders);
    let provided = folders.provide(&store, &build, variant)?;
    let shown = if provided.kept {
        store.resolve(&RuntimeRef {
            name: build.name.clone(),
            build: Some(provided.build.clone()),
        })?
    } else {
        build
    };
    let row = Provided {
        folders: folders.root().display().to_string(),
        variant,
        runtime: shown.name.clone(),
        build: shown.build.to_string(),
        dir: provided.dir.display().to_string(),
        hold: store.hold_file(&shown).display().to_string(),
        kept: provided.kept,
    };
    if provided.kept {
        app.out.warning(format!(
            "{variant} stays on {} {}: a game runs on it",
            row.runtime, row.build
        ))?;
    }
    #[cfg(feature = "json")]
    if app.out.json {
        app.out.json(&row)?;
        return Ok(0);
    }
    let what = if provided.changed {
        "provided"
    } else {
        "up to date"
    };
    app.out
        .info(format!("{} {} for umu: {}", row.runtime, row.build, what))?;
    app.out.line(format!("UMU_FOLDERS_PATH={}", row.folders))?;
    app.out.line(format!("hold: {}", row.hold))?;
    Ok(0)
}
