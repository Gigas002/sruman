//! Install transactions (pacman's `-S`): resolve targets and dependencies
//! against the sync index and the store, show a summary, then for each item
//! download into the package cache, verify against `SHA256SUMS`, extract
//! into staging, and commit into the store.
//!
//! Each item commits atomically (a rename), and a failure stops the
//! transaction with earlier items installed, as pacman does. Dependencies
//! come first, so a failure never leaves a runtime without what it needs.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    catalog::{Arch, Catalog, CatalogError, RuntimeDef},
    fetch::{Fetch, FetchError},
    manifest::Diff,
    progress::{CancelFlag, Phase, Progress},
    store::{BuildMeta, Installed, MakeCurrent, Source, Store, StoreError, StoreLock},
    sync::{self, ArchiveInfo, SyncDir, SyncError},
    version::{BuildId, Channel, RuntimeRef},
};

mod download;
pub mod extract;

pub use download::{Fetched, download};
pub use extract::ExtractError;

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("`{0}` isn't in the sync index; run `sruman -Sy` first")]
    NotSynced(String),
    #[error("`{name}` has no {channel} build on the server")]
    NoChannelBuild { name: String, channel: Channel },
    #[error("{name} {build} has no {arch} archive")]
    NoArchive {
        name: String,
        build: BuildId,
        arch: &'static str,
    },
    #[error("this machine's architecture has no Steam Linux Runtime")]
    UnsupportedArch,
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error("{url}: checksum mismatch (expected {expected}, got {actual})")]
    Checksum {
        url: String,
        expected: String,
        actual: String,
    },
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("cancelled")]
    Cancelled,
}

impl From<crate::progress::Cancelled> for InstallError {
    fn from(_: crate::progress::Cancelled) -> Self {
        Self::Cancelled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Named on the command line.
    Explicit,
    /// Pulled in by `depends`.
    Dependency,
}

/// One build to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxItem {
    pub name: String,
    pub build: BuildId,
    pub reason: Reason,
    pub archive: ArchiveInfo,
    pub url: String,
    /// The runtime's `current` build before the transaction.
    pub current: Option<BuildId>,
}

impl TxItem {
    /// `sniper 3.0.20261110.268881`, for progress and messages.
    pub fn label(&self) -> String {
        format!("{} {}", self.name, self.build)
    }

    /// `pkg/<name>-<build>-<archive file>`.
    pub fn cache_path(&self, pkg_cache: &Path) -> PathBuf {
        pkg_cache.join(format!(
            "{}-{}-{}",
            self.name, self.build, self.archive.file
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// That exact build is installed. (Builds never change, so there is
    /// nothing to reinstall; `-Qkk` checks an installed build's files.)
    Installed,
    /// A dependency with some build installed already.
    DependencySatisfied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub build: Option<BuildId>,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxOptions {
    pub channel: Channel,
    pub arch: Arch,
    /// `-w`: download and verify into the cache, install nothing.
    pub download_only: bool,
}

impl TxOptions {
    /// Stable channel, the host's architecture.
    pub fn new() -> Result<Self, InstallError> {
        Ok(Self {
            channel: Channel::Stable,
            arch: Arch::current().ok_or(InstallError::UnsupportedArch)?,
            download_only: false,
        })
    }
}

/// A resolved install, ready for [`Transaction::summary`] and
/// [`Transaction::commit`].
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    /// In install order (dependencies first).
    pub items: Vec<TxItem>,
    pub skipped: Vec<Skipped>,
    download_only: bool,
}

/// What a transaction will do, for the `Proceed?` prompt.
#[derive(Debug, Clone, Default)]
pub struct TxSummary {
    /// Bytes still to download (cached archives don't count).
    pub download_size: u64,
    /// Installed size of the new builds' packages, where the manifest is
    /// known (KiB).
    pub installed_size_kib: u64,
    /// Per item, the package changes against the runtime's current build,
    /// when both manifests are known.
    pub changes: Vec<(String, Diff)>,
}

/// How a commit went.
#[derive(Debug, Default)]
pub struct CommitReport {
    pub installed: Vec<Installed>,
    /// With `-w`: archives now verified in the cache.
    pub downloaded: Vec<PathBuf>,
    /// The item that failed, and why; later items weren't attempted.
    pub failed: Option<(String, InstallError)>,
    pub not_attempted: Vec<String>,
}

impl CommitReport {
    pub fn ok(&self) -> bool {
        self.failed.is_none()
    }
}

fn channel_build(
    index: &sync::SyncIndex,
    name: &str,
    channel: Channel,
) -> Result<BuildId, InstallError> {
    index
        .channels
        .get(&channel)
        .cloned()
        .ok_or_else(|| InstallError::NoChannelBuild {
            name: name.to_owned(),
            channel,
        })
}

impl Transaction {
    /// Resolves `targets` (pacman's `-S`): each runtime at its pinned build
    /// or the channel's, after any missing dependency. A pinned build the
    /// index doesn't know is looked up on the server.
    pub fn install(
        fetch: &dyn Fetch,
        sync_dir: &SyncDir,
        store: &Store,
        catalog: &Catalog,
        targets: &[RuntimeRef],
        options: &TxOptions,
    ) -> Result<Self, InstallError> {
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        let order = catalog.install_order(names.iter().copied())?;
        let mut tx = Self {
            download_only: options.download_only,
            ..Self::default()
        };
        for def in order {
            let explicit: Vec<&RuntimeRef> =
                targets.iter().filter(|t| t.name == def.name).collect();
            let installed = store.installed_of(&def.name)?;
            if explicit.is_empty() {
                if !installed.is_empty() {
                    tx.skipped.push(Skipped {
                        name: def.name.clone(),
                        build: store.current(&def.name),
                        reason: SkipReason::DependencySatisfied,
                    });
                    continue;
                }
                let index = sync_dir
                    .load(&def.name)
                    .ok_or_else(|| InstallError::NotSynced(def.name.clone()))?;
                let build = channel_build(&index, &def.name, options.channel)?;
                tx.push(
                    fetch,
                    sync_dir,
                    catalog,
                    store,
                    def,
                    build,
                    Reason::Dependency,
                    options,
                )?;
                continue;
            }
            for target in explicit {
                let build = match &target.build {
                    Some(build) => build.clone(),
                    None => {
                        let index = sync_dir
                            .load(&def.name)
                            .ok_or_else(|| InstallError::NotSynced(def.name.clone()))?;
                        channel_build(&index, &def.name, options.channel)?
                    }
                };
                if installed.iter().any(|i| i.build == build) {
                    tx.skipped.push(Skipped {
                        name: def.name.clone(),
                        build: Some(build),
                        reason: SkipReason::Installed,
                    });
                    continue;
                }
                if tx
                    .items
                    .iter()
                    .any(|i| i.name == def.name && i.build == build)
                {
                    continue;
                }
                tx.push(
                    fetch,
                    sync_dir,
                    catalog,
                    store,
                    def,
                    build,
                    Reason::Explicit,
                    options,
                )?;
            }
        }
        Ok(tx)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        fetch: &dyn Fetch,
        sync_dir: &SyncDir,
        catalog: &Catalog,
        store: &Store,
        def: &RuntimeDef,
        build: BuildId,
        reason: Reason,
        options: &TxOptions,
    ) -> Result<(), InstallError> {
        let index = sync_dir
            .load(&def.name)
            .ok_or_else(|| InstallError::NotSynced(def.name.clone()))?;
        let info = match index.builds.get(&build) {
            Some(info) => info.clone(),
            None => sync::add_build(fetch, catalog, sync_dir, &def.name, &build)?,
        };
        let archive =
            info.archives
                .get(&options.arch)
                .cloned()
                .ok_or_else(|| InstallError::NoArchive {
                    name: def.name.clone(),
                    build: build.clone(),
                    arch: options.arch.as_str(),
                })?;
        let url = format!("{}/{build}/{}", sync::images_url(def), archive.file);
        self.items.push(TxItem {
            name: def.name.clone(),
            current: store.current(&def.name),
            build,
            reason,
            archive,
            url,
        });
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Sizes and package changes, for the prompt.
    pub fn summary(&self, sync_dir: &SyncDir, store: &Store, pkg_cache: &Path) -> TxSummary {
        let mut summary = TxSummary::default();
        for item in &self.items {
            let cached = item.cache_path(pkg_cache);
            if !cached.is_file() {
                let size = item.archive.size.unwrap_or(0);
                let mut part = cached.into_os_string();
                part.push(".part");
                let have = fs::metadata(part).map_or(0, |m| m.len());
                summary.download_size += size.saturating_sub(have);
            }
            let new = sync_dir.manifest(&item.name, &item.build);
            if let Some(new) = &new {
                summary.installed_size_kib += new.installed_size_kib();
            }
            let old = item
                .current
                .as_ref()
                .and_then(|build| {
                    store
                        .resolve(&RuntimeRef {
                            name: item.name.clone(),
                            build: Some(build.clone()),
                        })
                        .ok()
                })
                .and_then(|installed| store.manifest(&installed));
            if let (Some(old), Some(new)) = (old, new) {
                summary.changes.push((item.name.clone(), old.diff(&new)));
            }
        }
        summary
    }

    /// Runs the transaction. Stops at the first failure or on cancel;
    /// whatever committed before stays installed.
    pub fn commit(
        &self,
        fetch: &dyn Fetch,
        store: &Store,
        lock: &StoreLock,
        pkg_cache: &Path,
        progress: &mut dyn Progress,
        cancel: &CancelFlag,
    ) -> CommitReport {
        let mut report = CommitReport::default();
        for (i, item) in self.items.iter().enumerate() {
            progress.begin(&item.label());
            let result = self.commit_item(item, fetch, store, lock, pkg_cache, progress, cancel);
            progress.end(result.is_ok());
            match result {
                Ok(Done::Installed(installed)) => report.installed.push(*installed),
                Ok(Done::Downloaded(path)) => report.downloaded.push(path),
                Err(e) => {
                    report.failed = Some((item.label(), e));
                    report.not_attempted = self.items[i + 1..].iter().map(TxItem::label).collect();
                    break;
                }
            }
        }
        report
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_item(
        &self,
        item: &TxItem,
        fetch: &dyn Fetch,
        store: &Store,
        lock: &StoreLock,
        pkg_cache: &Path,
        progress: &mut dyn Progress,
        cancel: &CancelFlag,
    ) -> Result<Done, InstallError> {
        cancel.check()?;
        let archive = item.cache_path(pkg_cache);
        download(
            fetch,
            &item.url,
            &item.archive.sha256,
            item.archive.size,
            &archive,
            progress,
            cancel,
        )?;
        if self.download_only {
            return Ok(Done::Downloaded(archive));
        }

        cancel.check()?;
        let staged = store.staging_dir(lock, &format!("{}-{}", item.name, item.build))?;
        let size = fs::metadata(&archive).ok().map(|m| m.len());
        progress.phase(Phase::Extract, size);
        let extracted = extract::extract_tar_xz(&archive, &staged, progress, cancel)
            .map_err(InstallError::from)
            .and_then(|_| {
                // pressure-vessel keeps its runtime copies in `var/`, which
                // must be on the build's filesystem.
                let var = staged.join("var");
                fs::create_dir_all(&var).map_err(|source| InstallError::Io {
                    context: format!("creating {}", var.display()),
                    source,
                })
            })
            .and_then(|()| cancel.check().map_err(InstallError::from));
        if let Err(e) = extracted {
            let _ = crate::store::remove_tree(&staged);
            return Err(e);
        }

        progress.phase(Phase::Commit, None);
        let meta = BuildMeta::new(
            &item.name,
            &item.build,
            Source::Download {
                url: item.url.clone(),
                sha256: item.archive.sha256.clone(),
            },
        );
        match store.commit(lock, &staged, meta, MakeCurrent::IfNewer) {
            Ok(installed) => Ok(Done::Installed(Box::new(installed))),
            Err(e) => {
                let _ = crate::store::remove_tree(&staged);
                Err(e.into())
            }
        }
    }
}

enum Done {
    Installed(Box<Installed>),
    Downloaded(PathBuf),
}
