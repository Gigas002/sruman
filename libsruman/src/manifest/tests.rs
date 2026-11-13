use super::*;

const STABLE: &str =
    include_str!("../../tests/fixtures/manifests/3.0.20261021.266025.sniper.manifest.dpkg");
const BETA: &str =
    include_str!("../../tests/fixtures/manifests/3.0.20261110.268881.sniper.manifest.dpkg");

#[test]
fn parses_a_real_manifest() {
    let manifest = Manifest::parse(STABLE).unwrap();
    assert_eq!(manifest.len(), 662);

    let adduser = manifest.get("adduser").unwrap();
    assert_eq!(adduser.version, "3.118+deb11u1");
    assert_eq!(adduser.arch, None);
    assert_eq!(adduser.source, None);
    assert_eq!(adduser.installed_size_kib, Some(849));

    let bsdutils = manifest.get("bsdutils").unwrap();
    assert_eq!(bsdutils.version, "1:2.36.1-8+deb11u2");
    assert_eq!(
        bsdutils.source.as_deref(),
        Some("util-linux (2.36.1-8+deb11u2)")
    );

    let sdl = manifest.get("libsdl3-0:i386").unwrap();
    assert_eq!(sdl.name, "libsdl3-0");
    assert_eq!(sdl.arch.as_deref(), Some("i386"));
    assert_eq!(sdl.key(), "libsdl3-0:i386");
    assert!(manifest.installed_size_kib() > 100_000);
}

#[test]
fn golden_diff_between_stable_and_beta() {
    let stable = Manifest::parse(STABLE).unwrap();
    let beta = Manifest::parse(BETA).unwrap();
    let diff = stable.diff(&beta);

    assert!(diff.added.is_empty());
    assert!(diff.removed.is_empty());
    let summary: Vec<String> = diff.summary().iter().map(ToString::to_string).collect();
    assert_eq!(
        summary,
        [
            "libsdl3-0 (amd64, i386)  3.4.18+ds-1+steamrt3.1+bsrt3.1 -> 3.4.20+ds-1+steamrt3.1+bsrt3.1",
            "libsteam-runtime-tools-0-0 (amd64, i386)  0.20261007.0+bsrt3.1 -> 0.20261110.0+bsrt3.1",
            "libsteam-runtime-tools-0-helpers (amd64, i386)  0.20261007.0+bsrt3.1 -> 0.20261110.0+bsrt3.1",
            "steam-runtime-tools-bin  0.20261007.0+bsrt3.1 -> 0.20261110.0+bsrt3.1",
            "steam-runtime-tools-minimal  0.20261007.0+bsrt3.1 -> 0.20261110.0+bsrt3.1",
            "steam-runtime-tools-ui  0.20261007.0+bsrt3.1 -> 0.20261110.0+bsrt3.1",
        ]
    );
    assert!(stable.diff(&stable).is_empty());
}

#[test]
fn diff_reports_added_and_removed() {
    let old = Manifest::parse("#h\na\t1\t\t\nb:amd64\t1\t\t\n").unwrap();
    let new = Manifest::parse("a\t2\t\t\nc\t3\t\t5\n").unwrap();
    let diff = old.diff(&new);

    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].key(), "b:amd64");
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.added[0].key(), "c");
    assert_eq!(diff.changed.len(), 1);
    let summary: Vec<String> = diff.summary().iter().map(ToString::to_string).collect();
    assert_eq!(
        summary,
        ["a  1 -> 2", "b (amd64)  1 (removed)", "c  (new) 3"]
    );
}

#[test]
fn search_ignores_case() {
    let manifest = Manifest::parse(STABLE).unwrap();
    let names: Vec<String> = manifest.search("SDL3").map(Package::key).collect();
    assert!(names.contains(&"libsdl3-0:amd64".to_owned()), "{names:?}");
    assert!(manifest.search("no-such-package").next().is_none());
}

#[test]
fn rejects_malformed_lines() {
    let cases = [
        ("a\t1\t\n", "expected 4 tab-separated fields, found 3"),
        ("a\t1\t\t\t\n", "expected 4 tab-separated fields, found 5"),
        (":amd64\t1\t\t\n", "empty package name"),
        ("a:\t1\t\t\n", "empty package name"),
        ("a\t\t\t\n", "empty version"),
        ("a\t1\t\tbig\n", "bad installed size `big`"),
        ("a\t1\t\t\na\t2\t\t\n", "duplicate package"),
    ];
    for (text, expected) in cases {
        let err = Manifest::parse(text).unwrap_err();
        assert!(err.to_string().contains(expected), "{text:?}: {err}");
    }
    assert_eq!(
        Manifest::parse("#x\n\na\t1\t\t\nb\t1\n").unwrap_err().line,
        4
    );
}
