use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    Issue, Issues, LoadedProfiles, Profile, Subject, ValidationContext, profile::file_issue,
    unknown_keys,
};
use crate::paths;

/// Keys a `[[game]]` entry may have; anything else gets a warning.
const KNOWN_KEYS: &[&str] = &["name", "exe", "profile", "args", "cwd", "env.*"];

/// One `[[game]]` entry in `games.toml`: lootrs' shape without `game_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// Unique id, used by `sruman <name>`.
    pub name: String,
    pub exe: PathBuf,
    /// Unset: `default_profile` from `config.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Put before the arguments given on the command line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Working directory; defaults to the executable's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Merged over the profile's env.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl Game {
    /// The profile this game runs with: its own, else `default_profile`.
    pub fn profile_name<'a>(&'a self, default_profile: Option<&'a str>) -> Option<&'a str> {
        self.profile.as_deref().or(default_profile)
    }

    /// Validates this game; `profile` is the profile it resolves to, if any.
    ///
    /// A missing `exe` or `cwd` is only a warning, so games on unmounted
    /// drives still load (as in lootrs).
    pub fn validate(&self, profile: Option<&Profile>, ctx: &ValidationContext) -> Vec<Issue> {
        let mut issues = Issues::new(Subject::Game(self.name.clone()));
        issues.name(&self.name);
        if profile.is_none() {
            match &self.profile {
                Some(name) => issues.error(format!("profile `{name}` does not exist")),
                None => {
                    issues.error("no `profile` set and no usable `default_profile` in config.toml")
                }
            }
        }
        if let Some(exe) = issues.absolute("exe", &self.exe, ctx.home)
            && !exe.is_file()
        {
            issues.warn(format!("`exe` not found: {}", exe.display()));
        }
        if let Some(cwd) = &self.cwd
            && let Some(cwd) = issues.absolute("cwd", cwd, ctx.home)
            && !cwd.is_dir()
        {
            issues.warn(format!("`cwd` not found: {}", cwd.display()));
        }
        issues.env(&self.env);
        issues.finish()
    }
}

/// `games.toml` as loaded, plus the issues found while loading.
#[derive(Debug, Clone, Default)]
pub struct LoadedGames {
    /// In file order; names are unique (later duplicates are dropped).
    pub games: Vec<Game>,
    /// An unreadable file, duplicate names and unknown keys.
    pub issues: Vec<Issue>,
}

impl LoadedGames {
    pub fn get(&self, name: &str) -> Option<&Game> {
        self.games.iter().find(|game| game.name == name)
    }

    /// Validates every game against `profiles` (implicit ones included).
    pub fn validate(
        &self,
        profiles: &LoadedProfiles,
        default_profile: Option<&str>,
        ctx: &ValidationContext,
    ) -> Vec<Issue> {
        self.games
            .iter()
            .flat_map(|game| {
                let profile = game
                    .profile_name(default_profile)
                    .and_then(|name| profiles.get(name, ctx.catalog));
                game.validate(profile.as_ref(), ctx)
            })
            .collect()
    }
}

#[derive(Deserialize)]
struct GamesFile {
    #[serde(default)]
    game: Vec<toml::Value>,
}

/// Loads `games.toml` (a missing file has no games). A broken entry is
/// reported and the others still load.
pub fn load_games(path: &Path) -> LoadedGames {
    let mut loaded = LoadedGames::default();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return loaded,
        Err(e) => {
            loaded
                .issues
                .push(file_issue(path, format!("can't read: {e}")));
            return loaded;
        }
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            loaded
                .issues
                .push(file_issue(path, format!("invalid games file: {e}")));
            return loaded;
        }
    };
    let mut top = Issues::new(Subject::File(path.to_path_buf()));
    unknown_keys(&value, &["game"], &mut top);
    loaded.issues.extend(top.finish());
    let file: GamesFile = match value.try_into() {
        Ok(file) => file,
        Err(e) => {
            loaded
                .issues
                .push(file_issue(path, format!("invalid games file: {e}")));
            return loaded;
        }
    };
    let mut seen = BTreeSet::new();
    for (index, entry) in file.game.into_iter().enumerate() {
        let label = entry
            .get("name")
            .and_then(toml::Value::as_str)
            .map_or_else(|| format!("#{}", index + 1), str::to_owned);
        let game: Game = match entry.clone().try_into() {
            Ok(game) => game,
            Err(e) => {
                loaded.issues.push(file_issue(
                    path,
                    format!("invalid `[[game]]` {label}: {}", e.message()),
                ));
                continue;
            }
        };
        let mut issues = Issues::new(Subject::Game(game.name.clone()));
        unknown_keys(&entry, KNOWN_KEYS, &mut issues);
        if !seen.insert(game.name.clone()) {
            issues.error("duplicate name; only the first entry is used");
            loaded.issues.extend(issues.finish());
            continue;
        }
        loaded.issues.extend(issues.finish());
        loaded.games.push(game);
    }
    loaded
}

/// What a bare run target means (D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget<'a> {
    Game(&'a Game),
    /// Absolute: `~` expanded and relative paths joined to the cwd.
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    #[error("no run target given")]
    Empty,
    /// `local` is set when a file of that name is in the current directory.
    #[error("no game '{name}'{}", local.as_ref().map(|_| format!(" (did you mean ./{name}?)")).unwrap_or_default())]
    UnknownGame {
        name: String,
        local: Option<PathBuf>,
    },
}

/// Resolves a bare target the way a shell does: with a `/` or a leading
/// `~` it's a path, otherwise a game name. A name never runs a file, even
/// one in the current directory; that only adds a `./name` hint.
pub fn resolve_target<'a>(
    target: &str,
    games: &'a [Game],
    cwd: &Path,
    home: &Path,
) -> Result<RunTarget<'a>, TargetError> {
    if target.is_empty() {
        return Err(TargetError::Empty);
    }
    if target.contains('/') || target.starts_with('~') {
        return Ok(RunTarget::Path(
            cwd.join(paths::expand_tilde(Path::new(target), home)),
        ));
    }
    if let Some(game) = games.iter().find(|game| game.name == target) {
        return Ok(RunTarget::Game(game));
    }
    let candidate = cwd.join(target);
    Err(TargetError::UnknownGame {
        name: target.to_owned(),
        local: candidate.is_file().then_some(candidate),
    })
}

impl fmt::Display for RunTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Game(game) => write!(f, "game `{}`", game.name),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}
