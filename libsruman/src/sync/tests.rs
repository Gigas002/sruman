use super::*;
use crate::{catalog::RuntimeOverride, testutil::MemFetch};

const BASE: &str = "https://fake";
const OLD: &str = "3.0.20261021.266025";
const NEW: &str = "3.0.20261110.268881";

fn fixture(path: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(path),
    )
    .unwrap()
}

/// [`MemFetch`] with paths relative to [`BASE`].
#[derive(Default)]
struct Server(MemFetch);

impl Server {
    fn put(&self, path: &str, data: impl Into<Vec<u8>>) {
        self.0.put(&format!("{BASE}{path}"), data);
    }

    fn remove(&self, path: &str) {
        self.0.remove(&format!("{BASE}{path}"));
    }

    fn take_log(&self) -> Vec<String> {
        self.0.take_log()
    }
}

impl std::ops::Deref for Server {
    type Target = MemFetch;

    fn deref(&self) -> &MemFetch {
        &self.0
    }
}

fn catalog() -> Catalog {
    let overrides: Vec<RuntimeOverride> = Catalog::builtin()
        .names()
        .map(|name| toml::from_str(&format!("name = '{name}'\nbase_url = '{BASE}/'")).unwrap())
        .collect();
    Catalog::builtin().with_overrides(&overrides).unwrap()
}

fn sums(build: &str, archive: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hex = |data: &[u8]| -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let p = "com.valvesoftware.SteamRuntime.Platform-amd64,i386-sniper";
    format!(
        "{} *BUILD_ID.txt\n{} *SteamLinuxRuntime_sniper.tar.xz\n{} *{p}.manifest.dpkg\n{} *{p}.os-release.txt\n\
         {} *com.valvesoftware.SteamRuntime.Sdk-amd64,i386-sniper.manifest.dpkg\n{} *sources/x.dsc\n",
        hex(build.as_bytes()),
        hex(archive),
        hex(b"m"),
        hex(b"o"),
        hex(b"s"),
        hex(b"x"),
    )
}

/// Publishes a sniper build on the fake server.
fn publish(server: &Server, build: &str, manifest: &str) {
    let dir = format!("/steamrt3/images/{build}");
    let archive = format!("archive {build}").into_bytes();
    let p = "com.valvesoftware.SteamRuntime.Platform-amd64,i386-sniper";
    server.put(&format!("{dir}/SHA256SUMS"), sums(build, &archive));
    server.put(&format!("{dir}/SteamLinuxRuntime_sniper.tar.xz"), archive);
    server.put(
        &format!("{dir}/{p}.manifest.dpkg"),
        fixture(&format!("manifests/{manifest}")),
    );
    server.put(
        &format!("{dir}/{p}.os-release.txt"),
        fixture("server/sniper.os-release.txt").replace(OLD, build),
    );
    server.put(
        &format!("{dir}/com.valvesoftware.SteamRuntime.Sdk-amd64,i386-sniper.manifest.dpkg"),
        "sdk",
    );
}

fn server() -> Server {
    let server = Server::default();
    publish(&server, OLD, &format!("{OLD}.sniper.manifest.dpkg"));
    publish(&server, NEW, &format!("{NEW}.sniper.manifest.dpkg"));
    server.put(
        "/steamrt3/images/update-timestamp.txt",
        "2026-11-10T18:36:01+00:00\n",
    );
    server.put(
        "/steamrt3/images/latest-public-stable.txt",
        format!("{OLD}\n"),
    );
    server.put(
        "/steamrt3/images/latest-public-beta.txt",
        format!("{NEW}\n"),
    );
    server.put(
        "/steamrt3/images/latest-container-runtime-depot.txt",
        format!("{OLD}\n"),
    );
    server.put(
        "/steamrt3/images/latest-steam-client-general-availability.txt",
        "\n",
    );
    server
}

fn only(name: &str) -> RefreshOptions {
    RefreshOptions {
        force: false,
        only: vec![name.into()],
    }
}

fn id(text: &str) -> BuildId {
    text.parse().unwrap()
}

#[test]
fn refresh_records_channels_builds_and_manifests() {
    let server = server();
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    assert_eq!(results.len(), 1);
    let RefreshOutcome::Updated { changed } = &results[0].1 else {
        panic!("{results:?}");
    };
    assert_eq!(changed, &Channel::ALL.to_vec());

    let index = dir.load("sniper").unwrap();
    assert_eq!(index.images_url, format!("{BASE}/steamrt3/images"));
    assert_eq!(index.update_timestamp, "2026-11-10T18:36:01+00:00");
    assert_eq!(index.channels[&Channel::Stable], id(OLD));
    assert_eq!(index.channels[&Channel::Beta], id(NEW));
    assert_eq!(index.channels[&Channel::Steam], id(OLD));
    assert_eq!(index.builds.len(), 2);

    let (build, info) = index.channel_build(Channel::Stable).unwrap();
    assert_eq!(build, &id(OLD));
    let archive = &info.archives[&Arch::X86_64];
    assert_eq!(archive.file, "SteamLinuxRuntime_sniper.tar.xz");
    assert_eq!(archive.size, Some(format!("archive {OLD}").len() as u64));
    assert_eq!(archive.sha256.len(), 64);
    assert_eq!(info.os_release["PRETTY_NAME"], "Steam Runtime 3 (sniper)");
    assert_eq!(info.os_release["BUILD_ID"], OLD);
    assert!(info.has_manifest);

    // The platform manifest was saved, not the SDK one.
    let old = dir.manifest("sniper", &id(OLD)).unwrap();
    let new = dir.manifest("sniper", &id(NEW)).unwrap();
    assert!(!old.diff(&new).summary().is_empty());
}

#[test]
fn unchanged_timestamp_costs_one_request() {
    let server = server();
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    refresh(&*server, &catalog(), &dir, &only("sniper"));
    server.take_log();

    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    assert!(matches!(results[0].1, RefreshOutcome::UpToDate));
    assert_eq!(
        server.take_log(),
        [format!("GET {BASE}/steamrt3/images/update-timestamp.txt")]
    );

    // -Syy re-reads the channels but not the builds it already knows.
    let forced = RefreshOptions {
        force: true,
        ..only("sniper")
    };
    let results = refresh(&*server, &catalog(), &dir, &forced);
    let RefreshOutcome::Updated { changed } = &results[0].1 else {
        panic!("{results:?}");
    };
    assert!(changed.is_empty());
    let log = server.take_log();
    assert!(log.iter().all(|r| r.ends_with(".txt")), "{log:#?}");
}

#[test]
fn a_new_build_is_fetched_and_dropped_builds_pruned() {
    let server = server();
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    refresh(&*server, &catalog(), &dir, &only("sniper"));

    // Stable moves to NEW; OLD is no longer referenced.
    server.put(
        "/steamrt3/images/update-timestamp.txt",
        "2026-11-12T00:00:00+00:00",
    );
    server.put("/steamrt3/images/latest-public-stable.txt", NEW);
    server.put("/steamrt3/images/latest-container-runtime-depot.txt", NEW);
    server.take_log();
    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    let RefreshOutcome::Updated { changed } = &results[0].1 else {
        panic!("{results:?}");
    };
    assert_eq!(changed, &[Channel::Stable, Channel::Steam]);
    assert!(
        !server.take_log().iter().any(|r| r.contains("SHA256SUMS")),
        "NEW was known"
    );
    let index = dir.load("sniper").unwrap();
    assert_eq!(index.builds.keys().collect::<Vec<_>>(), [&id(NEW)]);
    assert!(dir.manifest("sniper", &id(OLD)).is_none());
    assert!(dir.manifest("sniper", &id(NEW)).is_some());

    // A pinned build can be added back on purpose.
    let info = add_build(&*server, &catalog(), &dir, "sniper", &id(OLD)).unwrap();
    assert!(info.has_manifest);
    assert!(dir.load("sniper").unwrap().builds.contains_key(&id(OLD)));
    assert!(dir.manifest("sniper", &id(OLD)).is_some());
    // Known builds are not refetched.
    server.take_log();
    add_build(&*server, &catalog(), &dir, "sniper", &id(OLD)).unwrap();
    assert!(server.take_log().is_empty());
    assert!(matches!(
        add_build(&*server, &catalog(), &dir, "nope", &id(OLD)),
        Err(SyncError::UnknownRuntime(_))
    ));
}

#[test]
fn steam_channel_falls_back_and_missing_pointers_are_skipped() {
    let server = server();
    server.put(
        "/steamrt3/images/latest-container-runtime-depot.txt",
        "  \n",
    );
    server.put(
        "/steamrt3/images/latest-steam-client-general-availability.txt",
        NEW,
    );
    server.remove("/steamrt3/images/latest-public-beta.txt");
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    refresh(&*server, &catalog(), &dir, &only("sniper"));
    let index = dir.load("sniper").unwrap();
    assert_eq!(index.channels[&Channel::Steam], id(NEW));
    assert!(!index.channels.contains_key(&Channel::Beta));
}

#[test]
fn failures_are_per_runtime_and_keep_the_old_index() {
    let server = server();
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    let all = RefreshOptions::default();
    let results = refresh(&*server, &catalog(), &dir, &all);
    let names: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["scout", "scout-ldlp", "sniper", "soldier", "steamrt4"]
    );
    for (name, outcome) in &results {
        let ok = matches!(outcome, RefreshOutcome::Updated { .. });
        assert_eq!(ok, name == "sniper", "{name}: {outcome:?}");
    }
    // scout and scout-ldlp share steamrt1: one timestamp request.
    let log = server.take_log();
    let stamps = log
        .iter()
        .filter(|r| r.contains("steamrt1/images/update-timestamp"))
        .count();
    assert_eq!(stamps, 1, "{log:#?}");

    // A bad pointer fails sniper and leaves its index alone.
    let before = dir.load("sniper").unwrap();
    server.put("/steamrt3/images/update-timestamp.txt", "later");
    server.put("/steamrt3/images/latest-public-stable.txt", "garbage!");
    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    let RefreshOutcome::Failed(err) = &results[0].1 else {
        panic!("{results:?}");
    };
    assert!(err.to_string().contains("not a build id"), "{err}");
    assert_eq!(dir.load("sniper").unwrap(), before);
}

#[test]
fn build_without_its_archive_is_an_error() {
    let server = server();
    server.put(
        &format!("/steamrt3/images/{OLD}/SHA256SUMS"),
        format!("{} *other.tar.xz\n", "a".repeat(64)),
    );
    let err = fetch_build(&*server, catalog().get("sniper").unwrap(), &id(OLD), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("lists none of SteamLinuxRuntime_sniper.tar.xz"),
        "{err}"
    );
}

#[test]
fn broken_or_moved_index_is_refetched() {
    let server = server();
    let tmp = tempfile::tempdir().unwrap();
    let dir = SyncDir::new(tmp.path());
    refresh(&*server, &catalog(), &dir, &only("sniper"));
    fs::write(tmp.path().join("sniper.toml"), "broken = [").unwrap();
    assert!(dir.load("sniper").is_none());
    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    assert!(matches!(results[0].1, RefreshOutcome::Updated { .. }));

    // A different base URL doesn't trust the old timestamp.
    let mut index = dir.load("sniper").unwrap();
    index.images_url = "https://elsewhere/steamrt3/images".into();
    dir.save(&index).unwrap();
    let results = refresh(&*server, &catalog(), &dir, &only("sniper"));
    assert!(matches!(results[0].1, RefreshOutcome::Updated { .. }));
}

#[test]
fn sha256sums_forms() {
    let a = "A".repeat(64);
    let text = format!(
        "{a} *x.tar.xz\n{} y.txt\nshort *z\n{a} \n\n",
        "b".repeat(64)
    );
    let sums = parse_sha256sums(&text);
    assert_eq!(sums.len(), 2);
    assert_eq!(sums["x.tar.xz"], "a".repeat(64));
    assert_eq!(sums["y.txt"], "b".repeat(64));
}

#[test]
fn os_release_forms() {
    let parsed = parse_os_release(&fixture("server/sniper.os-release.txt"));
    assert_eq!(parsed["ID"], "steamrt");
    assert_eq!(parsed["VERSION_CODENAME"], "sniper");
    assert_eq!(parsed["VERSION"], "3 (sniper)");
    let parsed = parse_os_release("# c\nA='q'\nB=\"x \\\"y\\\"\"\n=z\nnoequals\n");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed["A"], "q");
    assert_eq!(parsed["B"], "x \"y\"");
}

#[test]
fn autoindex_of_the_real_server() {
    let html = fixture("server/steamrt3-images.html");
    let all = parse_autoindex(&html, None);
    let sniper = parse_autoindex(&html, Some(3));
    assert!(
        all.len() > sniper.len() && sniper.len() > 10,
        "{} {}",
        all.len(),
        sniper.len()
    );
    assert!(sniper.iter().all(|b| b.parts()[0] == 3));
    assert!(sniper.windows(2).all(|w| w[0] < w[1]));
    assert!(sniper.contains(&id(OLD)) && sniper.contains(&id(NEW)));
    assert_eq!(parse_autoindex(&html, Some(0))[0], id("0.20220119.0"));
}

#[test]
fn autoindex_is_tolerant() {
    let html = "<a href='3.0.1/'>a</a> <A HREF=\"x\"> <a href=\"./3.0.2/\">\
                <a href=\"../\"> <a href=\"3.0.1/\"> <a href=\"latest.txt\"> <a href=\"a/3.0.9/\">\
                <a href=\"3.0/\"> <a href=3.0.5/> <a href=\"3.x.1/\">";
    assert_eq!(parse_autoindex(html, None), [id("3.0.1"), id("3.0.2")]);
    assert!(parse_autoindex("", None).is_empty());
}

#[test]
fn list_builds_reads_the_images_page() {
    let server = server();
    server.put("/steamrt3/images/", fixture("server/steamrt3-images.html"));
    let builds = list_builds(&*server, catalog().get("sniper").unwrap(), Some(3)).unwrap();
    assert!(builds.contains(&id(NEW)));
}
