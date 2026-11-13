use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::{
    catalog::RuntimeOverride,
    progress::NoProgress,
    sync::RefreshOptions,
    testutil::{MemFetch, MemFile, TarEntry, sha256_hex, tar, tar_xz},
};

// ---- extraction -------------------------------------------------------

fn extract(entries: &[TarEntry]) -> (tempfile::TempDir, Result<String, ExtractError>) {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let result = extract::extract_tar(&tar(entries)[..], &dest, &CancelFlag::new(), &mut || {});
    (tmp, result)
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn extracts_valves_shapes_and_strips_the_top_level() {
    use TarEntry::*;
    let (tmp, result) = extract(&[
        Dir("SteamLinuxRuntime_sniper/"),
        File("SteamLinuxRuntime_sniper/run", b"#!/bin/sh\n", 0o755),
        Dir("SteamLinuxRuntime_sniper/pressure-vessel/"),
        Dir("SteamLinuxRuntime_sniper/pressure-vessel/bin/"),
        File(
            "SteamLinuxRuntime_sniper/pressure-vessel/libexec/tool",
            b"x",
            0o644,
        ),
        Symlink(
            "SteamLinuxRuntime_sniper/pressure-vessel/bin/tool",
            "../libexec/tool",
        ),
        File("./SteamLinuxRuntime_sniper/setuid", b"x", 0o4755),
        File("SteamLinuxRuntime_sniper/readonly", b"x", 0o444),
        Dir("SteamLinuxRuntime_sniper/rodir/"),
        File("SteamLinuxRuntime_sniper/rodir/inner", b"x", 0o644),
    ]);
    assert_eq!(result.unwrap(), "SteamLinuxRuntime_sniper");
    let dest = tmp.path().join("dest");
    assert_eq!(fs::read(dest.join("run")).unwrap(), b"#!/bin/sh\n");
    assert_eq!(mode(&dest.join("run")), 0o755);
    assert_eq!(mode(&dest.join("setuid")), 0o755, "setuid dropped");
    assert_eq!(mode(&dest.join("readonly")), 0o644, "owner can write");
    assert_eq!(
        fs::read_link(dest.join("pressure-vessel/bin/tool")).unwrap(),
        Path::new("../libexec/tool")
    );
    assert_eq!(
        fs::read(dest.join("pressure-vessel/bin/tool")).unwrap(),
        b"x"
    );
    let mtime = fs::metadata(dest.join("run")).unwrap().modified().unwrap();
    assert_eq!(
        mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        1_700_000_000
    );
}

#[test]
fn refuses_everything_valve_never_ships() {
    use TarEntry::*;
    let cases: Vec<(&str, Vec<TarEntry>)> = vec![
        (
            "dotdot path",
            vec![Dir("t/"), File("t/../../evil", b"x", 0o644)],
        ),
        ("absolute path", vec![File("/tmp/evil", b"x", 0o644)]),
        (
            "absolute link",
            vec![Dir("t/"), Symlink("t/l", "/etc/passwd")],
        ),
        (
            "escaping link",
            vec![Dir("t/"), Symlink("t/a/l", "../../x")],
        ),
        (
            "dotdot after a name",
            vec![Dir("t/"), Symlink("t/s", "."), Symlink("t/l", "s/..")],
        ),
        ("empty link", vec![Dir("t/"), Symlink("t/l", "")]),
        (
            "through a link",
            vec![
                Dir("t/"),
                Dir("t/d/"),
                Symlink("t/l", "d"),
                File("t/l/x", b"x", 0o644),
            ],
        ),
        (
            "over a link",
            vec![Dir("t/"), Symlink("t/l", "x"), File("t/l", b"x", 0o644)],
        ),
        (
            "hardlink",
            vec![Dir("t/"), File("t/a", b"x", 0o644), Hardlink("t/b", "t/a")],
        ),
        ("fifo", vec![Dir("t/"), Fifo("t/p")]),
        ("two tops", vec![Dir("a/"), Dir("b/")]),
        ("top is a file", vec![File("t", b"x", 0o644)]),
        (
            "duplicate file",
            vec![
                Dir("t/"),
                File("t/a", b"1", 0o644),
                File("t/a", b"2", 0o644),
            ],
        ),
        ("empty", vec![]),
    ];
    for (name, entries) in cases {
        let (tmp, result) = extract(&entries);
        assert!(result.is_err(), "{name} was accepted");
        let outside: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(outside, ["dest"], "{name} wrote outside");
    }
}

#[test]
fn symlink_rule() {
    for (rel, link, ok) in [
        ("l", "x", true),
        ("a/l", "../x", true),
        ("a/b/l", "../../x/y", true),
        ("a/l", "./../x", true),
        ("l", "../x", false),
        ("a/l", "../../x", false),
        ("l", "s/..", false),
        ("l", "s/../x", false),
        ("l", "/x", false),
        ("l", "", false),
    ] {
        assert_eq!(
            extract::symlink_stays_inside(Path::new(rel), Path::new(link)),
            ok,
            "{rel} -> {link}"
        );
    }
    assert_eq!(
        extract::split_top(Path::new("./a/b")),
        Some(("a".into(), "b".into()))
    );
    assert_eq!(
        extract::split_top(Path::new("a")),
        Some(("a".into(), PathBuf::new()))
    );
    assert!(extract::split_top(Path::new("a/../b")).is_none());
}

#[derive(Default)]
struct Bytes {
    phases: Vec<(Phase, Option<u64>)>,
    done: u64,
}

impl Progress for Bytes {
    fn begin(&mut self, _: &str) {}
    fn phase(&mut self, phase: Phase, total: Option<u64>) {
        self.phases.push((phase, total));
        self.done = 0;
    }
    fn advance(&mut self, n: u64) {
        self.done += n;
    }
    fn end(&mut self, _: bool) {}
}

#[test]
fn tar_xz_reports_compressed_bytes_and_cancels() {
    use TarEntry::*;
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("a.tar.xz");
    let big = vec![42u8; 300_000];
    fs::write(&archive, tar_xz(&[Dir("t/"), File("t/big", &big, 0o644)])).unwrap();
    let dest = tmp.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let mut progress = Bytes::default();
    extract::extract_tar_xz(&archive, &dest, &mut progress, &CancelFlag::new()).unwrap();
    assert_eq!(progress.done, fs::metadata(&archive).unwrap().len());
    assert_eq!(fs::read(dest.join("big")).unwrap(), big);

    let cancel = CancelFlag::new();
    cancel.cancel();
    let dest2 = tmp.path().join("dest2");
    fs::create_dir(&dest2).unwrap();
    let err = extract::extract_tar_xz(&archive, &dest2, &mut NoProgress, &cancel).unwrap_err();
    assert!(matches!(err, ExtractError::Cancelled(_)), "{err}");

    fs::write(&archive, b"not xz").unwrap();
    assert!(
        extract::extract_tar_xz(&archive, &dest2, &mut NoProgress, &CancelFlag::new()).is_err()
    );
}

// ---- download ---------------------------------------------------------

const URL: &str = "https://fake/f.tar.xz";

fn data() -> Vec<u8> {
    (0..100_000u32).map(|i| (i % 251) as u8).collect()
}

fn get(fetch: &MemFetch, dest: &Path, sha: &str) -> Result<Fetched, InstallError> {
    download(
        fetch,
        URL,
        sha,
        Some(100_000),
        dest,
        &mut NoProgress,
        &CancelFlag::new(),
    )
}

#[test]
fn download_verifies_and_reuses_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("pkg/f.tar.xz");
    let fetch = MemFetch::default();
    fetch.put(URL, data());
    let sha = sha256_hex(&data());

    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Downloaded);
    assert_eq!(fs::read(&dest).unwrap(), data());
    fetch.take_log();
    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Cached);
    assert!(fetch.take_log().is_empty());

    // A corrupted cache entry is replaced.
    fs::write(&dest, b"rotten").unwrap();
    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Downloaded);
    assert_eq!(fs::read(&dest).unwrap(), data());
}

#[test]
fn checksum_mismatch_keeps_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("f.tar.xz");
    let fetch = MemFetch::default();
    fetch.put(URL, data());
    let err = get(&fetch, &dest, &"0".repeat(64)).unwrap_err();
    assert!(matches!(err, InstallError::Checksum { .. }), "{err}");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn a_cut_download_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("f.tar.xz");
    let fetch = MemFetch::default();
    fetch.put_file(
        URL,
        MemFile {
            data: data(),
            cut_after: Some(30_000),
            ..MemFile::default()
        },
    );
    let sha = sha256_hex(&data());
    assert!(matches!(
        get(&fetch, &dest, &sha),
        Err(InstallError::Fetch(_))
    ));
    let part = tmp.path().join("f.tar.xz.part");
    assert_eq!(fs::metadata(&part).unwrap().len(), 30_000);

    fetch.take_log();
    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Downloaded);
    assert_eq!(fetch.take_log(), [format!("GET {URL} @30000")]);
    assert_eq!(fs::read(&dest).unwrap(), data());
    assert!(!part.exists());
}

#[test]
fn a_stale_part_starts_over_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("f.tar.xz");
    fs::write(tmp.path().join("f.tar.xz.part"), vec![9u8; 5000]).unwrap();
    let fetch = MemFetch::default();
    fetch.put(URL, data());
    assert_eq!(
        get(&fetch, &dest, &sha256_hex(&data())).unwrap(),
        Fetched::Downloaded
    );
    assert_eq!(
        fetch.take_log(),
        [format!("GET {URL} @5000"), format!("GET {URL}")]
    );
    assert_eq!(fs::read(&dest).unwrap(), data());
}

#[test]
fn ignored_range_and_oversized_part_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("f.tar.xz");
    let part = tmp.path().join("f.tar.xz.part");
    let fetch = MemFetch::default();
    fetch.put_file(
        URL,
        MemFile {
            data: data(),
            ignore_range: true,
            ..MemFile::default()
        },
    );
    let sha = sha256_hex(&data());
    fs::write(&part, &data()[..1234]).unwrap();
    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Downloaded);
    assert_eq!(fs::read(&dest).unwrap(), data());

    fs::remove_file(&dest).unwrap();
    fs::write(&part, vec![0u8; 200_000]).unwrap();
    fetch.take_log();
    assert_eq!(get(&fetch, &dest, &sha).unwrap(), Fetched::Downloaded);
    assert_eq!(fetch.take_log(), [format!("GET {URL}")]);
}

#[test]
fn cancel_keeps_the_part() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("f.tar.xz");
    let fetch = MemFetch::default();
    fetch.put(URL, data());
    let cancel = CancelFlag::new();
    cancel.cancel();
    let err = download(&fetch, URL, "x", None, &dest, &mut NoProgress, &cancel).unwrap_err();
    assert!(matches!(err, InstallError::Cancelled), "{err}");
    assert!(!dest.exists());
}

// ---- transactions -----------------------------------------------------

const BASE: &str = "https://fake";
const SOLDIER: &str = "2.0.20261021.266042";
const SCOUT: &str = "1.0.20261021.266054";
const SNIPER_OLD: &str = "3.0.20261021.266025";
const SNIPER_NEW: &str = "3.0.20261110.268881";

fn fixture(path: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(path),
    )
    .unwrap()
}

fn id(text: &str) -> BuildId {
    text.parse().unwrap()
}

fn catalog() -> Catalog {
    let overrides: Vec<RuntimeOverride> = Catalog::builtin()
        .names()
        .map(|name| toml::from_str(&format!("name = '{name}'\nbase_url = '{BASE}'")).unwrap())
        .collect();
    Catalog::builtin().with_overrides(&overrides).unwrap()
}

/// A container archive like Valve's, with `files/manifest.dpkg`.
fn container(top: &str, suite: &str, build: &str, manifest: &str) -> Vec<u8> {
    use TarEntry::*;
    let platform = format!("{top}/{suite}_platform_{build}/");
    let files = format!("{platform}files/");
    let manifest_path = format!("{files}manifest.dpkg");
    let run = format!("{top}/run");
    tar_xz(&[
        Dir(&format!("{top}/")),
        File(&run, b"#!/bin/sh\nexec \"$@\"\n", 0o755),
        Dir(&platform),
        Dir(&files),
        File(&manifest_path, manifest.as_bytes(), 0o644),
    ])
}

struct Fake {
    fetch: MemFetch,
    _tmp: tempfile::TempDir,
    sync: SyncDir,
    store: Store,
    pkg: PathBuf,
}

impl Fake {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Self {
            fetch: MemFetch::default(),
            sync: SyncDir::new(tmp.path().join("sync")),
            store: Store::open(tmp.path().join("runtimes")),
            pkg: tmp.path().join("pkg"),
            _tmp: tmp,
        };
        let manifest = fixture(&format!("manifests/{SNIPER_OLD}.sniper.manifest.dpkg"));
        let new_manifest = fixture(&format!("manifests/{SNIPER_NEW}.sniper.manifest.dpkg"));
        fake.publish(
            "steamrt2",
            "soldier",
            SOLDIER,
            "SteamLinuxRuntime_soldier.tar.xz",
            &manifest,
        );
        fake.publish(
            "steamrt1",
            "scout",
            SCOUT,
            "SteamLinuxRuntime.tar.xz",
            &manifest,
        );
        fake.publish(
            "steamrt3",
            "sniper",
            SNIPER_OLD,
            "SteamLinuxRuntime_sniper.tar.xz",
            &manifest,
        );
        fake.publish(
            "steamrt3",
            "sniper",
            SNIPER_NEW,
            "SteamLinuxRuntime_sniper.tar.xz",
            &new_manifest,
        );
        for (dir, build) in [
            ("steamrt1", SCOUT),
            ("steamrt2", SOLDIER),
            ("steamrt3", SNIPER_OLD),
        ] {
            fake.fetch
                .put(&format!("{BASE}/{dir}/images/update-timestamp.txt"), "t1");
            fake.fetch.put(
                &format!("{BASE}/{dir}/images/latest-public-stable.txt"),
                build,
            );
        }
        fake.fetch.put(
            &format!("{BASE}/steamrt3/images/latest-public-beta.txt"),
            SNIPER_NEW,
        );
        let options = RefreshOptions {
            force: false,
            only: vec!["scout".into(), "soldier".into(), "sniper".into()],
        };
        for (name, outcome) in sync::refresh(&fake.fetch, &catalog(), &fake.sync, &options) {
            assert!(
                matches!(outcome, sync::RefreshOutcome::Updated { .. }),
                "{name}: {outcome:?}"
            );
        }
        fake
    }

    fn publish(&self, dir: &str, suite: &str, build: &str, archive: &str, manifest: &str) {
        let top = archive.trim_end_matches(".tar.xz");
        let data = container(top, suite, build, manifest);
        let base = format!("{BASE}/{dir}/images/{build}");
        let p = format!("com.valvesoftware.SteamRuntime.Platform-amd64,i386-{suite}");
        self.fetch.put(
            &format!("{base}/SHA256SUMS"),
            format!(
                "{} *{archive}\n{} *{p}.manifest.dpkg\n",
                sha256_hex(&data),
                sha256_hex(manifest.as_bytes())
            ),
        );
        self.fetch.put(&format!("{base}/{archive}"), data);
        self.fetch
            .put(&format!("{base}/{p}.manifest.dpkg"), manifest);
    }

    fn tx(&self, targets: &[&str], options: &TxOptions) -> Result<Transaction, InstallError> {
        let targets: Vec<RuntimeRef> = targets.iter().map(|t| t.parse().unwrap()).collect();
        Transaction::install(
            &self.fetch,
            &self.sync,
            &self.store,
            &catalog(),
            &targets,
            options,
        )
    }

    fn commit(&self, tx: &Transaction) -> CommitReport {
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

fn opts() -> TxOptions {
    TxOptions {
        channel: Channel::Stable,
        arch: Arch::X86_64,
        download_only: false,
    }
}

fn labels(tx: &Transaction) -> Vec<String> {
    tx.items
        .iter()
        .map(|i| format!("{}{:?}", i.label(), i.reason))
        .collect()
}

#[test]
fn installs_with_dependencies_first() {
    let fake = Fake::new();
    let tx = fake.tx(&["scout"], &opts()).unwrap();
    assert_eq!(
        labels(&tx),
        [
            format!("soldier {SOLDIER}Dependency"),
            format!("scout {SCOUT}Explicit")
        ]
    );
    let summary = tx.summary(&fake.sync, &fake.store, &fake.pkg);
    assert!(summary.download_size > 0);
    assert!(summary.installed_size_kib > 0);
    assert!(
        summary.changes.is_empty(),
        "nothing installed to compare with"
    );

    let report = fake.commit(&tx);
    assert!(report.ok(), "{:?}", report.failed);
    assert_eq!(report.installed.len(), 2);
    for (name, build) in [("soldier", SOLDIER), ("scout", SCOUT)] {
        let installed = fake.store.resolve(&name.parse().unwrap()).unwrap();
        assert_eq!(installed.build, id(build));
        assert!(installed.dir.join("run").is_file());
        assert!(installed.dir.join("var").is_dir());
        assert!(matches!(
            installed.meta.unwrap().source,
            Source::Download { .. }
        ));
        assert_eq!(fake.store.current(name), Some(id(build)));
    }
    let staging = fake._tmp.path().join("runtimes/.staging");
    assert_eq!(
        fs::read_dir(&staging).map_or(0, |d| d.count()),
        0,
        "staging is empty"
    );

    // Everything is there now.
    let again = fake.tx(&["scout"], &opts()).unwrap();
    assert!(again.is_empty());
    let reasons: Vec<_> = again
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.clone()))
        .collect();
    assert_eq!(
        reasons,
        [
            ("soldier", SkipReason::DependencySatisfied),
            ("scout", SkipReason::Installed)
        ]
    );
    // Cached archives cost nothing to download again.
    assert_eq!(
        tx.summary(&fake.sync, &fake.store, &fake.pkg).download_size,
        0
    );
}

#[test]
fn upgrade_summary_shows_package_changes_and_pins_work() {
    let fake = Fake::new();
    assert!(fake.commit(&fake.tx(&["sniper"], &opts()).unwrap()).ok());
    let beta = TxOptions {
        channel: Channel::Beta,
        ..opts()
    };
    let tx = fake.tx(&["sniper"], &beta).unwrap();
    assert_eq!(labels(&tx), [format!("sniper {SNIPER_NEW}Explicit")]);
    assert_eq!(tx.items[0].current, Some(id(SNIPER_OLD)));
    let summary = tx.summary(&fake.sync, &fake.store, &fake.pkg);
    assert_eq!(summary.changes.len(), 1);
    assert!(!summary.changes[0].1.summary().is_empty());
    assert!(fake.commit(&tx).ok());
    assert_eq!(fake.store.current("sniper"), Some(id(SNIPER_NEW)));

    // Pinning the older build again is a no-op; current stays the newest.
    assert!(
        fake.tx(&[&format!("sniper={SNIPER_OLD}")], &opts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_pinned_build_outside_the_index_is_looked_up() {
    let fake = Fake::new();
    let pinned = format!("sniper={SNIPER_NEW}");
    // The beta channel brought it in; forget it, as after a refresh that
    // moved beta on.
    let mut index = fake.sync.load("sniper").unwrap();
    index.builds.remove(&id(SNIPER_NEW));
    fake.sync.save(&index).unwrap();
    fake.fetch.take_log();

    let tx = fake.tx(&[&pinned], &opts()).unwrap();
    assert_eq!(labels(&tx), [format!("sniper {SNIPER_NEW}Explicit")]);
    let log = fake.fetch.take_log();
    assert!(
        log.iter()
            .any(|r| r.ends_with(&format!("{SNIPER_NEW}/SHA256SUMS"))),
        "{log:#?}"
    );
    assert!(
        fake.sync
            .load("sniper")
            .unwrap()
            .builds
            .contains_key(&id(SNIPER_NEW))
    );
}

#[test]
fn missing_index_build_or_arch_are_errors() {
    let fake = Fake::new();
    assert!(matches!(
        fake.tx(&["steamrt4"], &opts()),
        Err(InstallError::NotSynced(_))
    ));
    assert!(matches!(
        fake.tx(&["nope"], &opts()),
        Err(InstallError::Catalog(_))
    ));
    let arm = TxOptions {
        arch: Arch::Arm64,
        ..opts()
    };
    assert!(matches!(
        fake.tx(&["sniper"], &arm),
        Err(InstallError::NoArchive { .. })
    ));
    let steam = TxOptions {
        channel: Channel::Steam,
        ..opts()
    };
    assert!(matches!(
        fake.tx(&["sniper"], &steam),
        Err(InstallError::NoChannelBuild { .. })
    ));
    assert!(matches!(
        fake.tx(&["sniper=3.0.1"], &opts()),
        Err(InstallError::Sync(_))
    ));
}

#[test]
fn a_failure_keeps_earlier_items_and_cleans_staging() {
    let fake = Fake::new();
    let mut tx = fake.tx(&["scout"], &opts()).unwrap();
    tx.items[1].archive.sha256 = "0".repeat(64);
    let report = fake.commit(&tx);
    let (label, err) = report.failed.as_ref().unwrap();
    assert_eq!(label, &format!("scout {SCOUT}"));
    assert!(matches!(err, InstallError::Checksum { .. }), "{err}");
    assert_eq!(report.installed.len(), 1);
    assert!(fake.store.current("soldier").is_some());
    assert!(fake.store.current("scout").is_none());

    // A broken archive (bad entry) fails in extraction and leaves no staging.
    let mut tx = fake.tx(&["sniper"], &opts()).unwrap();
    let bad = tar_xz(&[TarEntry::Dir("t/"), TarEntry::Symlink("t/l", "/etc")]);
    fake.fetch.put(&tx.items[0].url, bad.clone());
    tx.items[0].archive.sha256 = sha256_hex(&bad);
    let report = fake.commit(&tx);
    assert!(
        matches!(report.failed, Some((_, InstallError::Extract(_)))),
        "{:?}",
        report.failed
    );
    let staging = fake._tmp.path().join("runtimes/.staging");
    let left = fs::read_dir(&staging).map_or(0, |d| d.count());
    assert_eq!(left, 0);
    assert!(fake.store.current("sniper").is_none());
}

#[test]
fn not_attempted_items_are_listed() {
    let fake = Fake::new();
    let mut tx = fake.tx(&["scout"], &opts()).unwrap();
    tx.items[0].archive.sha256 = "0".repeat(64);
    let report = fake.commit(&tx);
    assert_eq!(report.not_attempted, [format!("scout {SCOUT}")]);
    assert!(report.installed.is_empty());
}

#[test]
fn download_only_fills_the_cache() {
    let fake = Fake::new();
    let options = TxOptions {
        download_only: true,
        ..opts()
    };
    let tx = fake.tx(&["sniper"], &options).unwrap();
    let report = fake.commit(&tx);
    assert!(report.ok());
    assert_eq!(report.downloaded, [tx.items[0].cache_path(&fake.pkg)]);
    assert!(report.downloaded[0].is_file());
    assert!(fake.store.current("sniper").is_none());

    // The real install then downloads nothing.
    fake.fetch.take_log();
    assert!(fake.commit(&fake.tx(&["sniper"], &opts()).unwrap()).ok());
    assert!(!fake.fetch.take_log().iter().any(|r| r.ends_with(".tar.xz")));
}

#[test]
fn cancel_before_commit_installs_nothing() {
    let fake = Fake::new();
    let tx = fake.tx(&["sniper"], &opts()).unwrap();
    let lock = fake.store.lock().unwrap();
    let cancel = CancelFlag::new();
    cancel.cancel();
    let report = tx.commit(
        &fake.fetch,
        &fake.store,
        &lock,
        &fake.pkg,
        &mut NoProgress,
        &cancel,
    );
    assert!(matches!(report.failed, Some((_, InstallError::Cancelled))));
    assert!(fake.store.current("sniper").is_none());
}

/// Extracts a real archive from Valve and checks it with its own
/// `mtree.txt.gz` at content depth. Run with
/// `SRUMAN_REAL_ARCHIVE=/path/SteamLinuxRuntime_sniper.tar.xz cargo test -p libsruman -- --ignored real_archive`.
#[test]
#[ignore = "needs a downloaded archive in SRUMAN_REAL_ARCHIVE"]
fn real_archive_extracts_and_verifies() {
    let archive =
        PathBuf::from(std::env::var_os("SRUMAN_REAL_ARCHIVE").expect("SRUMAN_REAL_ARCHIVE"));
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path());
    let lock = store.lock().unwrap();
    let staged = store.staging_dir(&lock, "real").unwrap();
    let mut progress = Bytes::default();
    let started = std::time::Instant::now();
    let top =
        extract::extract_tar_xz(&archive, &staged, &mut progress, &CancelFlag::new()).unwrap();
    eprintln!("extracted {top} in {:?}", started.elapsed());
    assert_eq!(progress.done, fs::metadata(&archive).unwrap().len());
    let build = fs::read_dir(&staged)
        .unwrap()
        .flatten()
        .find_map(|e| {
            e.file_name()
                .to_str()?
                .split_once("_platform_")
                .map(|(_, b)| b.to_owned())
        })
        // scout-on-soldier has no platform of its own.
        .unwrap_or_else(|| "0.0.1".to_owned());
    let meta = BuildMeta::new(
        "real",
        &id(&build),
        Source::Import {
            from: archive.clone(),
        },
    );
    let installed = store
        .commit(&lock, &staged, meta, crate::store::MakeCurrent::Always)
        .unwrap();
    let started = std::time::Instant::now();
    let Some(report) = store
        .verify(&installed, crate::mtree::Depth::Content)
        .unwrap()
    else {
        let links = walk_count_links(&installed.dir);
        eprintln!("no mtree.txt.gz; {links} symlinks extracted");
        assert!(links > 0);
        return;
    };
    eprintln!(
        "verified {} entries in {:?}",
        report.checked,
        started.elapsed()
    );
    assert!(
        report.problems.is_empty(),
        "{:#?}",
        &report.problems[..report.problems.len().min(20)]
    );
    assert!(report.checked > 1000);
    if build != "0.0.1" {
        assert!(
            store
                .manifest(&installed)
                .is_some_and(|m| m.iter().count() > 100)
        );
    }
}

fn walk_count_links(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            let t = e.file_type().unwrap();
            if t.is_symlink() {
                1
            } else if t.is_dir() {
                walk_count_links(&e.path())
            } else {
                0
            }
        })
        .sum()
}

// ---- upgrades ---------------------------------------------------------

impl Fake {
    /// Points sniper's stable channel at `build` and refreshes.
    fn move_stable(&self, build: &str, stamp: &str) {
        self.fetch.put(
            &format!("{BASE}/steamrt3/images/latest-public-stable.txt"),
            build,
        );
        self.fetch.put(
            &format!("{BASE}/steamrt3/images/update-timestamp.txt"),
            stamp,
        );
        let only = RefreshOptions {
            force: false,
            only: vec!["sniper".into()],
        };
        sync::refresh(&self.fetch, &catalog(), &self.sync, &only);
    }

    fn up(&self, which: &Upgrade, downgrade: bool) -> Result<Transaction, InstallError> {
        Transaction::upgrade(
            &self.fetch,
            &self.sync,
            &self.store,
            &catalog(),
            which,
            &opts(),
            &UpgradeOptions {
                allow_downgrade: downgrade,
            },
        )
    }
}

fn skipped(tx: &Transaction) -> Vec<(String, SkipReason)> {
    tx.skipped
        .iter()
        .map(|s| (s.name.clone(), s.reason.clone()))
        .collect()
}

#[test]
fn upgrade_installs_the_newer_channel_build() {
    let fake = Fake::new();
    assert!(
        fake.commit(&fake.tx(&["scout", "sniper"], &opts()).unwrap())
            .ok()
    );
    let tx = fake.up(&Upgrade::All, false).unwrap();
    assert!(tx.is_empty());
    assert_eq!(
        skipped(&tx),
        // Install order: soldier before scout.
        [
            ("soldier".into(), SkipReason::UpToDate),
            ("scout".into(), SkipReason::UpToDate),
            ("sniper".into(), SkipReason::UpToDate),
        ]
    );

    fake.move_stable(SNIPER_NEW, "t2");
    let tx = fake.up(&Upgrade::All, false).unwrap();
    assert_eq!(labels(&tx), [format!("sniper {SNIPER_NEW}Explicit")]);
    assert_eq!(tx.items[0].make_current, MakeCurrent::IfNewer);
    assert!(fake.commit(&tx).ok());
    assert_eq!(fake.store.current("sniper"), Some(id(SNIPER_NEW)));
    // The old build stays installed (`clean` decides about it).
    assert_eq!(fake.store.installed_of("sniper").unwrap().len(), 2);
}

#[test]
fn going_back_needs_uu() {
    let fake = Fake::new();
    fake.move_stable(SNIPER_NEW, "t2");
    assert!(fake.commit(&fake.tx(&["sniper"], &opts()).unwrap()).ok());
    fake.move_stable(SNIPER_OLD, "t3");

    let tx = fake.up(&Upgrade::All, false).unwrap();
    assert_eq!(skipped(&tx), [("sniper".into(), SkipReason::UpToDate)]);

    let tx = fake.up(&Upgrade::All, true).unwrap();
    assert_eq!(labels(&tx), [format!("sniper {SNIPER_OLD}Explicit")]);
    assert_eq!(tx.items[0].make_current, MakeCurrent::Always);
    assert!(fake.commit(&tx).ok());
    assert_eq!(fake.store.current("sniper"), Some(id(SNIPER_OLD)));

    // Forward again: NEW is installed, so only `current` moves.
    fake.move_stable(SNIPER_NEW, "t4");
    let tx = fake
        .up(&Upgrade::Named(vec!["sniper".into()]), false)
        .unwrap();
    assert!(tx.items.is_empty());
    assert_eq!(
        tx.switches,
        [Switch {
            name: "sniper".into(),
            from: Some(id(SNIPER_OLD)),
            to: id(SNIPER_NEW),
        }]
    );
    fake.fetch.take_log();
    let report = fake.commit(&tx);
    assert!(report.ok());
    assert_eq!(report.switched.len(), 1);
    assert!(fake.fetch.take_log().is_empty(), "nothing downloaded");
    assert_eq!(fake.store.current("sniper"), Some(id(SNIPER_NEW)));

    // And back to the installed OLD with -uu: a switch too.
    fake.move_stable(SNIPER_OLD, "t5");
    let tx = fake.up(&Upgrade::All, true).unwrap();
    assert_eq!(tx.switches.len(), 1);
    assert!(tx.items.is_empty());
}

#[test]
fn upgrade_named_errors_and_all_skips() {
    let fake = Fake::new();
    assert!(matches!(
        fake.up(&Upgrade::Named(vec!["nope".into()]), false),
        Err(InstallError::Catalog(_))
    ));
    assert!(matches!(
        fake.up(&Upgrade::Named(vec!["steamrt4".into()]), false),
        Err(InstallError::NotSynced(_))
    ));
    // Named but not installed: installs the channel build.
    let tx = fake
        .up(&Upgrade::Named(vec!["sniper".into()]), false)
        .unwrap();
    assert_eq!(labels(&tx), [format!("sniper {SNIPER_OLD}Explicit")]);

    // All: an installed runtime without an index, and one the catalog
    // doesn't know, are skipped.
    let lock = fake.store.lock().unwrap();
    for (name, build) in [("steamrt4", "4.0.1"), ("custom", "1.0.1")] {
        let staged = fake.store.staging_dir(&lock, name).unwrap();
        crate::store::tests::fake_container(&staged, name, build);
        let meta = BuildMeta::new(name, &id(build), Source::Import { from: "/x".into() });
        fake.store
            .commit(&lock, &staged, meta, MakeCurrent::IfNewer)
            .unwrap();
    }
    drop(lock);
    let tx = fake.up(&Upgrade::All, false).unwrap();
    assert!(tx.is_empty());
    assert_eq!(
        skipped(&tx),
        [
            ("custom".into(), SkipReason::NotSynced),
            ("steamrt4".into(), SkipReason::NotSynced)
        ]
    );
}

#[test]
fn download_only_upgrade_moves_nothing() {
    let fake = Fake::new();
    assert!(fake.commit(&fake.tx(&["sniper"], &opts()).unwrap()).ok());
    fake.move_stable(SNIPER_NEW, "t2");
    let options = TxOptions {
        download_only: true,
        ..opts()
    };
    let tx = Transaction::upgrade(
        &fake.fetch,
        &fake.sync,
        &fake.store,
        &catalog(),
        &Upgrade::All,
        &options,
        &UpgradeOptions::default(),
    )
    .unwrap();
    let report = fake.commit(&tx);
    assert_eq!(report.downloaded.len(), 1);
    assert_eq!(fake.store.current("sniper"), Some(id(SNIPER_OLD)));
}
