//! Against Valve's real server; ignored by default (network, ~190 MB).
//! See docs/RUNTIMES.md, "Testing against the real server".

#![cfg(feature = "ureq")]

use std::path::PathBuf;

use libsruman::{
    catalog::{Arch, Catalog},
    fetch::UreqFetch,
    install::{Transaction, TxOptions},
    mtree::Depth,
    progress::{CancelFlag, NoProgress},
    store::Store,
    sync::{self, RefreshOptions, RefreshOutcome, SyncDir},
    version::{Channel, RuntimeRef},
};

#[test]
#[ignore = "network: downloads from repo.steampowered.com"]
fn real_server_refresh_install_verify() {
    let runtime = std::env::var("SRUMAN_REAL_INSTALL").unwrap_or_else(|_| "sniper".into());
    let tmp = tempfile::tempdir().unwrap();
    let root =
        std::env::var_os("SRUMAN_REAL_DIR").map_or_else(|| tmp.path().to_path_buf(), PathBuf::from);
    let fetch = UreqFetch::new();
    let catalog = Catalog::builtin();
    let sync_dir = SyncDir::new(root.join("sync"));

    for (name, outcome) in sync::refresh(&fetch, &catalog, &sync_dir, &RefreshOptions::default()) {
        assert!(
            matches!(
                outcome,
                RefreshOutcome::Updated { .. } | RefreshOutcome::UpToDate
            ),
            "{name}: {outcome:?}"
        );
        let index = sync_dir.load(&name).unwrap();
        let (build, info) = index
            .channel_build(Channel::Stable)
            .expect("a stable build");
        eprintln!(
            "{name}: stable {build}, {} MiB, {}",
            info.archives[&Arch::X86_64].size.unwrap_or(0) >> 20,
            info.os_release
                .get("PRETTY_NAME")
                .map_or("?", String::as_str),
        );
        if name != "scout" && name != "scout-ldlp" {
            assert!(info.has_manifest, "{name}");
        }
    }
    let def = catalog.get("sniper").unwrap();
    let builds = sync::list_builds(&fetch, def, Some(3)).unwrap();
    assert!(builds.len() > 10, "{builds:?}");

    let store = Store::open(root.join("runtimes"));
    let targets: Vec<RuntimeRef> = vec![runtime.parse().unwrap()];
    let tx = Transaction::install(
        &fetch,
        &sync_dir,
        &store,
        &catalog,
        &targets,
        &TxOptions::new().unwrap(),
    )
    .unwrap();
    let lock = store.lock().unwrap();
    let started = std::time::Instant::now();
    let report = tx.commit(
        &fetch,
        &store,
        &lock,
        &root.join("pkg"),
        &mut NoProgress,
        &CancelFlag::new(),
    );
    assert!(report.ok(), "{:?}", report.failed);
    eprintln!(
        "installed {:?} in {:?}",
        report
            .installed
            .iter()
            .map(|i| i.build.to_string())
            .collect::<Vec<_>>(),
        started.elapsed()
    );
    for installed in &report.installed {
        if let Some(verify) = store.verify(installed, Depth::Content).unwrap() {
            assert!(
                verify.problems.is_empty(),
                "{}: {:#?}",
                installed.name,
                verify.problems
            );
            eprintln!(
                "{} {}: {} entries verified",
                installed.name, installed.build, verify.checked
            );
        }
    }
}
