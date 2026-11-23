//! One handler per command, grouped like the CLI tables: runtimes, games
//! and running, profiles, doctor.

use super::{App, AppError, supervisor};
use crate::settings::{
    Command,
    command::{Profiles, Steam},
};

#[cfg(feature = "diag")]
pub mod doctor;
pub mod games;
pub mod profiles;
pub mod runtimes;
pub mod steam;
pub mod umu;

#[cfg(test)]
mod tests;

/// Runs `command`; returns the exit code.
pub fn dispatch(app: &mut App, command: &Command) -> Result<u8, AppError> {
    match command {
        #[cfg(feature = "download")]
        Command::Sync { force } => runtimes::sync(app, *force),
        #[cfg(feature = "download")]
        Command::Install {
            targets,
            needed,
            download_only,
        } => runtimes::install(app, targets, *needed, *download_only),
        #[cfg(feature = "download")]
        Command::Upgrade {
            targets,
            sync,
            allow_downgrade,
            download_only,
        } => runtimes::upgrade(app, targets, *sync, *allow_downgrade, *download_only),
        #[cfg(feature = "download")]
        Command::Outdated { targets } => runtimes::outdated(app, targets),
        Command::Remove { targets, force } => runtimes::remove(app, targets, *force),
        #[cfg(feature = "download")]
        Command::Search { terms, packages } => runtimes::search(app, terms, *packages),
        Command::Info { targets, packages } => runtimes::info(app, targets, *packages),
        Command::Runtimes { quiet } => runtimes::runtimes(app, *quiet),
        #[cfg(feature = "download")]
        Command::Builds { runtime } => runtimes::builds(app, runtime),
        Command::Files { runtime } => runtimes::files(app, runtime),
        Command::Verify { targets, full } => runtimes::verify(app, targets, *full),
        #[cfg(feature = "download")]
        Command::Clean { all } => runtimes::clean(app, *all),
        Command::Import { sources, link } => runtimes::import(app, sources, *link),

        Command::Run(run) => supervisor::run(app, run),
        Command::Execute(execute) => supervisor::execute(app, execute),
        Command::Shell { profile, debug } => {
            let profile = profile
                .as_deref()
                .ok_or_else(|| AppError::Invalid("shell needs a profile".into()))?;
            supervisor::shell(app, profile, *debug)
        }
        Command::Status => games::status(app),
        Command::Stop { targets } => games::stop(app, targets),
        Command::Killall => games::killall(app),
        Command::List => games::list(app),
        Command::Show { game } => games::show(app, game),
        Command::Check { targets } => games::check(app, targets),
        Command::Logs { game, path_only } => games::logs(app, game, *path_only),
        Command::Init { profile } => games::init(app, profile),
        Command::Profiles(command) => match command {
            Profiles::List => profiles::list(app),
            Profiles::Show { name } => profiles::show(app, name),
            Profiles::New {
                name,
                runtime,
                mode,
                prefix,
            } => profiles::new(app, name, runtime, *mode, prefix.as_deref()),
            Profiles::Edit { name } => profiles::edit(app, name),
            Profiles::EditGames => profiles::edit_games(app),
            Profiles::Delete { names, prefix } => profiles::delete(app, names, *prefix),
        },
        #[cfg(feature = "diag")]
        Command::Doctor { runtimes } => doctor::doctor(app, runtimes),
        Command::Steam(command) => match command {
            Steam::Status => steam::status(app),
            Steam::Link { runtimes } => steam::link(app, runtimes),
            Steam::Unlink { runtimes } => steam::unlink(app, runtimes),
            Steam::Verify { runtimes, full } => steam::verify(app, runtimes, *full),
            Steam::Clean => steam::clean(app),
            Steam::Relocate { undo } => steam::relocate(app, *undo),
        },
        Command::ProvideUmu { runtime, app_id } => umu::provide(app, runtime.as_ref(), *app_id),
    }
}

/// Unix seconds as local `YYYY-MM-DD HH:MM`.
pub fn date(secs: u64) -> String {
    let Ok(secs) = i64::try_from(secs) else {
        return secs.to_string();
    };
    jiff::Timestamp::from_second(secs).map_or_else(
        |_| secs.to_string(),
        |ts| {
            ts.to_zoned_datetime(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}
