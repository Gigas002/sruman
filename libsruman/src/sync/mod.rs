//! The local sync index (pacman's sync databases): what Valve's server
//! offers, per runtime, refreshed by `-Sy`.
//!
//! For each runtime, `sync/<name>.toml` records the server's
//! `update-timestamp.txt`, which build each [`Channel`] points to, and per
//! build: the archive names, sizes and SHA-256 sums, and the runtime's
//! `os-release`. The package list (`manifest.dpkg`) of each build is kept
//! next to it as `sync/manifests/<name>-<build>.manifest.dpkg`.
//!
//! [`refresh`] costs one small request per runtime when nothing was
//! published (the timestamp is unchanged). Builds never change once
//! published, so a refresh only fetches builds it doesn't know yet.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{Arch, Catalog, RuntimeDef},
    fetch::{Fetch, FetchError, SMALL_FILE_LIMIT},
    manifest::Manifest,
    version::{BuildId, Channel},
};

#[cfg(test)]
mod tests;

/// Prefix of the per-build metadata files Valve publishes.
const PLATFORM_PREFIX: &str = "com.valvesoftware.SteamRuntime.Platform-";

/// One downloadable archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveInfo {
    pub file: String,
    /// Lowercase hex, from `SHA256SUMS`.
    pub sha256: String,
    /// From a `HEAD`; unknown if the server didn't say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// One published build of a runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub archives: BTreeMap<Arch, ArchiveInfo>,
    /// `os-release` of the runtime (`PRETTY_NAME`, `VERSION_ID`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub os_release: BTreeMap<String, String>,
    /// Whether `manifests/<name>-<build>.manifest.dpkg` was saved.
    #[serde(default)]
    pub has_manifest: bool,
}

/// `sync/<name>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncIndex {
    pub runtime: String,
    /// `<base_url>/<server_dir>/images`, so a changed base URL is noticed.
    pub images_url: String,
    /// The server's `update-timestamp.txt`, verbatim.
    pub update_timestamp: String,
    /// Unix seconds of the last successful refresh.
    pub refreshed_at: u64,
    #[serde(default)]
    pub channels: BTreeMap<Channel, BuildId>,
    /// Builds some channel points to, plus any added with [`add_build`]
    /// (a pinned install) since the channels last changed.
    #[serde(default)]
    pub builds: BTreeMap<BuildId, BuildInfo>,
}

impl SyncIndex {
    /// The build `channel` points to, with its info.
    pub fn channel_build(&self, channel: Channel) -> Option<(&BuildId, &BuildInfo)> {
        let id = self.channels.get(&channel)?;
        Some((id, self.builds.get(id)?))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error("{0}")]
    Server(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unknown runtime `{0}`")]
    UnknownRuntime(String),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SyncError + '_ {
    move |source| SyncError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The `sync/` directory.
#[derive(Debug, Clone)]
pub struct SyncDir {
    root: PathBuf,
}

impl SyncDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self, runtime: &str) -> PathBuf {
        self.root.join(format!("{runtime}.toml"))
    }

    fn manifest_path(&self, runtime: &str, build: &BuildId) -> PathBuf {
        self.root
            .join("manifests")
            .join(format!("{runtime}-{build}.manifest.dpkg"))
    }

    /// The saved index, if there is a readable one. A broken file counts as
    /// missing: the next refresh rewrites it.
    pub fn load(&self, runtime: &str) -> Option<SyncIndex> {
        let text = fs::read_to_string(self.index_path(runtime)).ok()?;
        toml::from_str(&text).ok()
    }

    pub fn save(&self, index: &SyncIndex) -> Result<(), SyncError> {
        let text = toml::to_string(index).map_err(|e| SyncError::Server(e.to_string()))?;
        write_atomic(&self.index_path(&index.runtime), text.as_bytes())
    }

    /// The package list of a build, if it was synced.
    pub fn manifest(&self, runtime: &str, build: &BuildId) -> Option<Manifest> {
        let text = fs::read_to_string(self.manifest_path(runtime, build)).ok()?;
        Manifest::parse(&text).ok()
    }

    fn save_manifest(&self, runtime: &str, build: &BuildId, text: &str) -> Result<(), SyncError> {
        write_atomic(&self.manifest_path(runtime, build), text.as_bytes())
    }

    /// Deletes manifests of builds no index mentions any more.
    fn prune_manifests(&self, keep: &[(String, BuildId)]) {
        let Ok(entries) = fs::read_dir(self.root.join("manifests")) else {
            return;
        };
        let keep: Vec<String> = keep
            .iter()
            .map(|(name, build)| format!("{name}-{build}.manifest.dpkg"))
            .collect();
        for entry in entries.flatten() {
            if !keep.iter().any(|k| entry.file_name() == k.as_str()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), SyncError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, data)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(io_err(path))
}

/// `<base_url>/<server_dir>/images`.
pub fn images_url(def: &RuntimeDef) -> String {
    format!(
        "{}/{}/images",
        def.base_url.trim_end_matches('/'),
        def.server_dir
    )
}

/// Parses `SHA256SUMS` (`<hex> *<file>` or `<hex>  <file>`) into file →
/// lowercase hex. Malformed lines are skipped.
pub fn parse_sha256sums(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once(' ')?;
            let file = file.trim_start_matches([' ', '*']);
            let ok = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            (ok && !file.is_empty()).then(|| (file.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// Parses `os-release` (`KEY=value`, values optionally quoted).
pub fn parse_os_release(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            if key.is_empty() || key.starts_with('#') {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            Some((key.to_owned(), value.replace("\\\"", "\"")))
        })
        .collect()
}

/// The arch tag in Valve's platform file names.
fn platform_tag(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "amd64,i386",
        Arch::Arm64 => "arm64",
    }
}

/// The platform metadata file for `arch` with `suffix`, among `files`:
/// `com.valvesoftware.SteamRuntime.Platform-amd64,i386-sniper.manifest.dpkg`.
fn platform_file<'a>(
    files: impl IntoIterator<Item = &'a String>,
    arch: Arch,
    suffix: &str,
) -> Option<&'a str> {
    let prefix = format!("{PLATFORM_PREFIX}{}-", platform_tag(arch));
    files
        .into_iter()
        .map(String::as_str)
        .find(|file| file.starts_with(&prefix) && file.ends_with(suffix) && !file.contains('/'))
}

/// Reads one build's metadata from the server: `SHA256SUMS`, archive
/// sizes, os-release and the manifest (saved into `dir` when given).
pub fn fetch_build(
    fetch: &dyn Fetch,
    def: &RuntimeDef,
    build: &BuildId,
    dir: Option<&SyncDir>,
) -> Result<BuildInfo, SyncError> {
    let base = format!("{}/{build}", images_url(def));
    let sums = parse_sha256sums(&fetch.get_text(&format!("{base}/SHA256SUMS"), SMALL_FILE_LIMIT)?);
    let mut archives = BTreeMap::new();
    for (arch, file) in &def.archives {
        let Some(sha256) = sums.get(file) else {
            continue;
        };
        let size = fetch.head(&format!("{base}/{file}"))?.len;
        archives.insert(
            *arch,
            ArchiveInfo {
                file: file.clone(),
                sha256: sha256.clone(),
                size,
            },
        );
    }
    if archives.is_empty() {
        return Err(SyncError::Server(format!(
            "{base}: SHA256SUMS lists none of {}",
            def.archives
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    // The metadata of the first listed arch that has it (x86_64 first).
    let mut os_release = BTreeMap::new();
    let mut has_manifest = false;
    for arch in archives.keys() {
        if os_release.is_empty()
            && let Some(file) = platform_file(sums.keys(), *arch, ".os-release.txt")
        {
            os_release =
                parse_os_release(&fetch.get_text(&format!("{base}/{file}"), SMALL_FILE_LIMIT)?);
        }
        if !has_manifest && let Some(file) = platform_file(sums.keys(), *arch, ".manifest.dpkg") {
            let text = fetch.get_text(&format!("{base}/{file}"), SMALL_FILE_LIMIT)?;
            if Manifest::parse(&text).is_ok() {
                if let Some(dir) = dir {
                    dir.save_manifest(&def.name, build, &text)?;
                }
                has_manifest = true;
            }
        }
    }
    Ok(BuildInfo {
        archives,
        os_release,
        has_manifest,
    })
}

/// The build a channel's pointer files name: the first that exists and
/// isn't empty.
fn read_channel(
    fetch: &dyn Fetch,
    def: &RuntimeDef,
    channel: Channel,
) -> Result<Option<BuildId>, SyncError> {
    let images = images_url(def);
    let names: Vec<&str> = match def.pointers.get(channel.as_str()) {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => channel.pointer_files().to_vec(),
    };
    for name in names {
        let url = format!("{images}/{name}.txt");
        let text = match fetch.get_text(&url, 4096) {
            Ok(text) => text,
            Err(e) if e.is_not_found() => continue,
            Err(e) => return Err(e.into()),
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        return text
            .parse()
            .map(Some)
            .map_err(|_| SyncError::Server(format!("{url}: not a build id: {text:?}")));
    }
    Ok(None)
}

#[derive(Debug, Clone, Default)]
pub struct RefreshOptions {
    /// `-Syy`: refetch the channels even when the timestamp is unchanged.
    pub force: bool,
    /// Only these runtimes; empty means every catalog runtime.
    pub only: Vec<String>,
}

/// What a refresh did for one runtime.
#[derive(Debug)]
pub enum RefreshOutcome {
    /// Nothing was published since the last refresh.
    UpToDate,
    /// Channels re-read; lists the channels whose build changed.
    Updated {
        changed: Vec<Channel>,
    },
    Failed(SyncError),
}

/// Refreshes the index of each runtime (pacman's `-Sy`). One runtime
/// failing doesn't stop the others.
pub fn refresh(
    fetch: &dyn Fetch,
    catalog: &Catalog,
    dir: &SyncDir,
    options: &RefreshOptions,
) -> Vec<(String, RefreshOutcome)> {
    let mut timestamps: HashMap<String, Result<String, String>> = HashMap::new();
    let mut results = Vec::new();
    for def in catalog.iter() {
        if !options.only.is_empty() && !options.only.contains(&def.name) {
            continue;
        }
        let images = images_url(def);
        // Runtimes on the same server directory (scout, scout-ldlp) share it.
        let stamp = timestamps
            .entry(images.clone())
            .or_insert_with(|| {
                fetch
                    .get_text(&format!("{images}/update-timestamp.txt"), 4096)
                    .map(|t| t.trim().to_owned())
                    .map_err(|e| e.to_string())
            })
            .clone();
        let outcome = match stamp {
            Err(message) => RefreshOutcome::Failed(SyncError::Server(message)),
            Ok(stamp) => match refresh_one(fetch, def, dir, &stamp, &images, options) {
                Ok(outcome) => outcome,
                Err(e) => RefreshOutcome::Failed(e),
            },
        };
        results.push((def.name.clone(), outcome));
    }
    let keep: Vec<(String, BuildId)> = catalog
        .iter()
        .filter_map(|def| dir.load(&def.name))
        .flat_map(|index| {
            index
                .builds
                .into_iter()
                .filter(|(_, info)| info.has_manifest)
                .map(move |(build, _)| (index.runtime.clone(), build))
        })
        .collect();
    dir.prune_manifests(&keep);
    results
}

fn refresh_one(
    fetch: &dyn Fetch,
    def: &RuntimeDef,
    dir: &SyncDir,
    stamp: &str,
    images: &str,
    options: &RefreshOptions,
) -> Result<RefreshOutcome, SyncError> {
    let old = dir.load(&def.name).filter(|old| old.images_url == images);
    if let Some(old) = &old
        && old.update_timestamp == stamp
        && !options.force
    {
        return Ok(RefreshOutcome::UpToDate);
    }
    let mut channels = BTreeMap::new();
    for channel in Channel::ALL {
        if let Some(build) = read_channel(fetch, def, channel)? {
            channels.insert(channel, build);
        }
    }
    let mut builds = BTreeMap::new();
    for build in channels.values() {
        if builds.contains_key(build) {
            continue;
        }
        let known = old
            .as_ref()
            .and_then(|old| old.builds.get(build))
            .filter(|info| !info.has_manifest || dir.manifest_path(&def.name, build).is_file());
        let info = match known {
            Some(info) => info.clone(),
            None => fetch_build(fetch, def, build, Some(dir))?,
        };
        builds.insert(build.clone(), info);
    }
    let changed = Channel::ALL
        .into_iter()
        .filter(|c| old.as_ref().and_then(|old| old.channels.get(c)) != channels.get(c))
        .collect();
    dir.save(&SyncIndex {
        runtime: def.name.clone(),
        images_url: images.to_owned(),
        update_timestamp: stamp.to_owned(),
        refreshed_at: now(),
        channels,
        builds,
    })?;
    Ok(RefreshOutcome::Updated { changed })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Records a build fetched on purpose (a pinned `-S sniper=<build>`) in the
/// index, so the install and `-Si` find it.
pub fn add_build(
    fetch: &dyn Fetch,
    catalog: &Catalog,
    dir: &SyncDir,
    runtime: &str,
    build: &BuildId,
) -> Result<BuildInfo, SyncError> {
    let def = catalog
        .get(runtime)
        .ok_or_else(|| SyncError::UnknownRuntime(runtime.to_owned()))?;
    let mut index = dir.load(runtime).ok_or_else(|| {
        SyncError::Server(format!(
            "no sync index for `{runtime}`; run `sruman -Sy` first"
        ))
    })?;
    if let Some(info) = index.builds.get(build) {
        return Ok(info.clone());
    }
    let info = fetch_build(fetch, def, build, Some(dir))?;
    index.builds.insert(build.clone(), info.clone());
    dir.save(&index)?;
    Ok(info)
}

/// Every build directory under `images/` (for `-Sl`), oldest first. Only
/// directories whose first number is `major` count when it's given (the
/// sniper server also has `0.2022…` builds from before the `3.0` naming).
pub fn list_builds(
    fetch: &dyn Fetch,
    def: &RuntimeDef,
    major: Option<u64>,
) -> Result<Vec<BuildId>, SyncError> {
    let html = fetch.get_text(&format!("{}/", images_url(def)), SMALL_FILE_LIMIT)?;
    Ok(parse_autoindex(&html, major))
}

/// Build ids among the `href="<id>/"` links of an autoindex page (nginx,
/// Apache or lighttpd), sorted and deduplicated. Anything else is ignored.
pub fn parse_autoindex(html: &str, major: Option<u64>) -> Vec<BuildId> {
    let mut builds: Vec<BuildId> = html
        .split("href=")
        .skip(1)
        .filter_map(|rest| {
            let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let target = rest[1..].split(quote).next()?;
            let name = target
                .strip_prefix("./")
                .unwrap_or(target)
                .strip_suffix('/')?;
            if name.contains('/') || name.matches('.').count() < 2 {
                return None;
            }
            name.parse::<BuildId>().ok()
        })
        .filter(|build| major.is_none_or(|major| build.parts().first() == Some(&major)))
        .collect();
    builds.sort();
    builds.dedup();
    builds
}
