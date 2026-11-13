//! End to end over real HTTP: `UreqFetch` against a local fake of Valve's
//! image server, through refresh, install, upgrade and the failure cases.

#![cfg(feature = "ureq")]

mod support {
    pub mod http;
    pub mod images;
}

use std::{fs, path::PathBuf};

use libsruman::{
    catalog::{Arch, Catalog, RuntimeOverride},
    fetch::{USER_AGENT, UreqFetch},
    install::{InstallError, Transaction, TxOptions, Upgrade, UpgradeOptions},
    progress::{CancelFlag, NoProgress},
    store::Store,
    sync::{self, RefreshOptions, RefreshOutcome, SyncDir},
    version::{BuildId, Channel, RuntimeRef},
};
use support::{
    http::{File, Server},
    images::Suite,
};

const SOLDIER: &str = "2.0.20261021.266042";
const SCOUT: &str = "1.0.20261021.266054";
const OLD: &str = "3.0.20261021.266025";
const NEW: &str = "3.0.20261110.268881";

fn manifest(build: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/manifests/{build}.sniper.manifest.dpkg",
        env!("CARGO_MANIFEST_DIR")
    );
    fs::read_to_string(path).unwrap()
}

fn id(text: &str) -> BuildId {
    text.parse().unwrap()
}

struct World {
    server: Server,
    fetch: UreqFetch,
    catalog: Catalog,
    sync: SyncDir,
    store: Store,
    pkg: PathBuf,
    _tmp: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let server = Server::start();
        let overrides: Vec<RuntimeOverride> = Catalog::builtin()
            .names()
            .map(|name| RuntimeOverride {
                name: name.to_owned(),
                description: None,
                base_url: Some(server.base.clone()),
                server_dir: None,
                archives: None,
                entry: None,
                depends: None,
                pointers: None,
            })
            .collect();
        let catalog = Catalog::builtin().with_overrides(&overrides).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let world = Self {
            fetch: UreqFetch::new(),
            catalog,
            sync: SyncDir::new(tmp.path().join("cache/sync")),
            store: Store::open(tmp.path().join("data/runtimes")),
            pkg: tmp.path().join("cache/pkg"),
            _tmp: tmp,
            server,
        };
        world.soldier().publish(SOLDIER, &manifest(OLD));
        world.soldier().point("latest-public-stable", SOLDIER, "t1");
        world.scout().publish(SCOUT, &manifest(OLD));
        world.scout().point("latest-public-stable", SCOUT, "t1");
        world.sniper().publish(OLD, &manifest(OLD));
        world.sniper().publish(NEW, &manifest(NEW));
        world.sniper().point("latest-public-stable", OLD, "t1");
        world
    }

    fn suite<'a>(&'a self, dir: &'a str, suite: &'a str, archive: &'a str) -> Suite<'a> {
        Suite {
            server: &self.server,
            dir,
            suite,
            archive,
        }
    }

    fn soldier(&self) -> Suite<'_> {
        self.suite("steamrt2", "soldier", "SteamLinuxRuntime_soldier.tar.xz")
    }

    fn scout(&self) -> Suite<'_> {
        self.suite("steamrt1", "scout", "SteamLinuxRuntime.tar.xz")
    }

    fn sniper(&self) -> Suite<'_> {
        self.suite("steamrt3", "sniper", "SteamLinuxRuntime_sniper.tar.xz")
    }

    fn refresh(&self, only: &[&str]) -> Vec<(String, RefreshOutcome)> {
        let options = RefreshOptions {
            force: false,
            only: only.iter().map(|s| (*s).to_owned()).collect(),
        };
        sync::refresh(&self.fetch, &self.catalog, &self.sync, &options)
    }

    fn options() -> TxOptions {
        TxOptions {
            channel: Channel::Stable,
            arch: Arch::X86_64,
            download_only: false,
        }
    }

    fn install(&self, targets: &[&str]) -> Result<Transaction, InstallError> {
        let targets: Vec<RuntimeRef> = targets.iter().map(|t| t.parse().unwrap()).collect();
        Transaction::install(
            &self.fetch,
            &self.sync,
            &self.store,
            &self.catalog,
            &targets,
            &Self::options(),
        )
    }

    fn upgrade(&self, downgrade: bool) -> Transaction {
        Transaction::upgrade(
            &self.fetch,
            &self.sync,
            &self.store,
            &self.catalog,
            &Upgrade::All,
            &Self::options(),
            &UpgradeOptions {
                allow_downgrade: downgrade,
            },
        )
        .unwrap()
    }

    fn commit(&self, tx: &Transaction) -> libsruman::install::CommitReport {
        let lock = self.store.lock().unwrap();
        tx.commit(
            &self.fetch,
            &self.store,
            &lock,
            &self.pkg,
            &mut NoProgress,
            &CancelFlag::new(),
        )
    }
}

#[test]
fn refresh_install_pin_upgrade_and_downgrade() {
    let w = World::new();
    let results = w.refresh(&[]);
    for (name, outcome) in &results {
        let updated = matches!(outcome, RefreshOutcome::Updated { .. });
        // scout-ldlp's archive and steamrt4 aren't on this fake server.
        assert_eq!(
            updated,
            ["scout", "sniper", "soldier"].contains(&name.as_str()),
            "{name}: {outcome:?}"
        );
    }

    // scout pulls in soldier.
    let tx = w.install(&["scout"]).unwrap();
    let names: Vec<_> = tx.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["soldier", "scout"]);
    assert!(tx.summary(&w.sync, &w.store, &w.pkg).download_size > 0);
    let report = w.commit(&tx);
    assert!(report.ok(), "{:?}", report.failed);
    let scout = w.store.resolve(&"scout".parse().unwrap()).unwrap();
    assert_eq!(scout.build, id(SCOUT));
    assert_eq!(
        fs::read_link(scout.dir.join("pressure-vessel/bin/tool")).unwrap(),
        PathBuf::from("../libexec/tool")
    );
    assert!(scout.dir.join("var").is_dir());

    // A pinned build, then the channel moves past it.
    assert!(
        w.commit(&w.install(&[&format!("sniper={OLD}")]).unwrap())
            .ok()
    );
    assert_eq!(w.store.current("sniper"), Some(id(OLD)));
    w.sniper().point("latest-public-stable", NEW, "t2");
    let results = w.refresh(&["sniper"]);
    let RefreshOutcome::Updated { changed } = &results[0].1 else {
        panic!("{results:?}");
    };
    assert_eq!(changed, &[Channel::Stable]);
    let tx = w.upgrade(false);
    assert_eq!(tx.items.len(), 1);
    let summary = tx.summary(&w.sync, &w.store, &w.pkg);
    assert_eq!(
        summary.changes.len(),
        1,
        "package diff against the installed build"
    );
    assert!(w.commit(&tx).ok());
    assert_eq!(w.store.current("sniper"), Some(id(NEW)));

    // The channel goes back: only -uu follows, by moving `current`.
    w.sniper().point("latest-public-stable", OLD, "t3");
    w.refresh(&["sniper"]);
    assert!(w.upgrade(false).is_empty());
    let tx = w.upgrade(true);
    assert_eq!(tx.switches.len(), 1);
    w.server.clear_log();
    assert!(w.commit(&tx).ok());
    assert!(w.server.log().is_empty(), "a switch downloads nothing");
    assert_eq!(w.store.current("sniper"), Some(id(OLD)));

    // Every request identified itself as sruman and nothing else.
    // (The log was cleared above; refresh once more to have requests.)
    w.sniper().point("latest-public-stable", OLD, "t4");
    w.refresh(&["sniper"]);
    let log = w.server.log();
    assert!(!log.is_empty());
    assert!(
        log.iter()
            .all(|r| r.user_agent.as_deref() == Some(USER_AGENT)),
        "{log:#?}"
    );
}

#[test]
fn a_truncated_download_resumes_with_range() {
    let w = World::new();
    w.refresh(&["sniper"]);
    let data = w.sniper().publish(OLD, &manifest(OLD));
    w.sniper().put_archive(
        OLD,
        File {
            cut_after: Some(data.len() / 2),
            ..File::new(data.clone())
        },
    );
    let tx = w.install(&["sniper"]).unwrap();
    let report = w.commit(&tx);
    assert!(
        matches!(report.failed, Some((_, InstallError::Fetch(_)))),
        "{:?}",
        report.failed
    );
    let part = fs::read_dir(&w.pkg)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .expect("a .part file");
    assert_eq!(part.metadata().unwrap().len(), (data.len() / 2) as u64);
    assert!(w.store.current("sniper").is_none());
    // The summary only counts what's missing.
    let left = tx.summary(&w.sync, &w.store, &w.pkg).download_size;
    assert_eq!(left, (data.len() - data.len() / 2) as u64);

    w.server.clear_log();
    let report = w.commit(&tx);
    assert!(report.ok(), "{:?}", report.failed);
    let ranges: Vec<_> = w.server.log().into_iter().filter_map(|r| r.range).collect();
    assert_eq!(ranges, [format!("bytes={}-", data.len() / 2)]);
    assert_eq!(w.store.current("sniper"), Some(id(OLD)));
}

#[test]
fn a_tampered_archive_is_rejected_and_not_kept() {
    let w = World::new();
    w.refresh(&["sniper"]);
    let mut data = w.sniper().publish(OLD, &manifest(OLD));
    let last = data.len() - 1;
    data[last] ^= 0xff;
    w.sniper().put_archive(OLD, File::new(data));
    let report = w.commit(&w.install(&["sniper"]).unwrap());
    assert!(
        matches!(report.failed, Some((_, InstallError::Checksum { .. }))),
        "{:?}",
        report.failed
    );
    assert!(w.store.installed().unwrap().is_empty());
    assert_eq!(fs::read_dir(&w.pkg).unwrap().count(), 0, "nothing cached");
}

#[test]
fn download_only_then_offline_install() {
    let w = World::new();
    w.refresh(&["sniper"]);
    let options = TxOptions {
        download_only: true,
        ..World::options()
    };
    let targets = ["sniper".parse().unwrap()];
    let tx =
        Transaction::install(&w.fetch, &w.sync, &w.store, &w.catalog, &targets, &options).unwrap();
    assert_eq!(w.commit(&tx).downloaded.len(), 1);

    // The archive is gone from the server; the cache is enough.
    w.server.remove(&w.sniper().archive_path(OLD));
    let report = w.commit(&w.install(&["sniper"]).unwrap());
    assert!(report.ok(), "{:?}", report.failed);
}
