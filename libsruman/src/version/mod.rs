//! Runtime build ids, runtime references and update channels.
//!
//! A [`BuildId`] is Valve's build id from `VERSION.txt`
//! (`3.0.20261021.266025`). A [`RuntimeRef`] names a runtime and optionally
//! pins a build, in pacman's `name=version` form: `sniper` or
//! `sniper=3.0.20261021.266025`.

use std::{cmp::Ordering, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("`{0}` is not a build id: expected dot-separated numbers like `3.0.20261021.266025`")]
    BuildId(String),
    #[error(
        "`{0}` is not a runtime name: use letters, digits, `.`, `_` and `-`, not starting with `-`"
    )]
    Name(String),
    #[error("`{0}` is not a channel: use `stable`, `beta` or `steam`")]
    Channel(String),
}

/// Valve's build id. Ordered numerically, part by part; a build id that is
/// a prefix of another sorts first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildId {
    text: String,
    parts: Vec<u64>,
}

impl BuildId {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The numeric parts, e.g. `[3, 0, 20261021, 266025]`.
    pub fn parts(&self) -> &[u64] {
        &self.parts
    }
}

impl FromStr for BuildId {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let err = || ParseError::BuildId(text.to_owned());
        if text.is_empty() {
            return Err(err());
        }
        let parts = text
            .split('.')
            .map(|part| {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(err());
                }
                part.parse::<u64>().map_err(|_| err())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            text: text.to_owned(),
            parts,
        })
    }
}

impl Ord for BuildId {
    fn cmp(&self, other: &Self) -> Ordering {
        // Equal numbers with different spellings (`01` vs `1`) still get a
        // total order consistent with `Eq`.
        self.parts
            .cmp(&other.parts)
            .then_with(|| self.text.cmp(&other.text))
    }
}

impl PartialOrd for BuildId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for BuildId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A runtime, optionally pinned to one build: `sniper` or `sniper=<build>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeRef {
    pub name: String,
    /// `None` follows the newest installed build.
    pub build: Option<BuildId>,
}

impl RuntimeRef {
    pub fn new(name: impl Into<String>, build: Option<BuildId>) -> Result<Self, ParseError> {
        let name = name.into();
        validate_runtime_name(&name)?;
        Ok(Self { name, build })
    }

    pub fn is_pinned(&self) -> bool {
        self.build.is_some()
    }
}

/// Runtime names follow the profile/game name rules: `[A-Za-z0-9._-]+`, not
/// `.`/`..`, not starting with `-`.
pub fn validate_runtime_name(name: &str) -> Result<(), ParseError> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ParseError::Name(name.to_owned()))
    }
}

impl FromStr for RuntimeRef {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        match text.split_once('=') {
            Some((name, build)) => Self::new(name, Some(build.parse()?)),
            None => Self::new(text, None),
        }
    }
}

impl fmt::Display for RuntimeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.build {
            Some(build) => write!(f, "{}={build}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// Which pointer file on Valve's server picks "the latest" build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// `latest-public-stable`.
    #[default]
    Stable,
    /// `latest-public-beta`.
    Beta,
    /// What the Steam client ships: `latest-container-runtime-depot`, or
    /// `latest-steam-client-general-availability` where that is empty
    /// (scout).
    Steam,
}

impl Channel {
    pub const ALL: [Self; 3] = [Self::Stable, Self::Beta, Self::Steam];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Steam => "steam",
        }
    }

    /// Pointer files (without `.txt`) under `images/`, tried in order until
    /// one holds a build id.
    pub fn pointer_files(self) -> &'static [&'static str] {
        match self {
            Self::Stable => &["latest-public-stable"],
            Self::Beta => &["latest-public-beta"],
            Self::Steam => &[
                "latest-container-runtime-depot",
                "latest-steam-client-general-availability",
            ],
        }
    }
}

impl FromStr for Channel {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == text.trim())
            .ok_or_else(|| ParseError::Channel(text.to_owned()))
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! serde_via_str {
    ($($ty:ty),*) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

serde_via_str!(BuildId, RuntimeRef, Channel);
