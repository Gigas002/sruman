//! Property tests (PLAN 5.5a/5.5c): every parser of outside data takes
//! arbitrary input without panicking, and the version and manifest types
//! keep their laws. They run in every `cargo test`; `PROPTEST_CASES=50000`
//! runs a longer pass.

use std::{cmp::Ordering, collections::BTreeMap, path::Path};

use libsruman::{
    manifest::Manifest,
    mtree,
    overlay::HostLibs,
    version::{BuildId, Channel, RuntimeRef},
};
use proptest::prelude::*;

/// Text that looks a little like the formats (separators, digits, markup),
/// so cases get past the first check more often than random bytes do.
fn texty() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            any::<char>().prop_map(String::from),
            prop::sample::select(vec![
                "\t",
                "\n",
                " ",
                ".",
                "=",
                "#",
                ":",
                "(",
                ")",
                "/",
                "\\",
                "\\040",
                "href=\"",
                "\">",
                "1",
                "3.0.",
                "sniper",
                "amd64",
                "  ",
                " => ",
                "type=file",
                "sha256=",
                "size=",
                "mode=",
                "link=",
                "./",
                "..",
            ])
            .prop_map(str::to_owned),
        ],
        0..64,
    )
    .prop_map(|parts| parts.concat())
}

fn build_id() -> impl Strategy<Value = BuildId> {
    prop::collection::vec(0u64..1_000_000_000, 1..6)
        .prop_map(|parts| {
            parts
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(".")
        })
        .prop_map(|text| text.parse().unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn text_parsers_never_panic(text in texty()) {
        let _ = text.parse::<BuildId>();
        let _ = text.parse::<RuntimeRef>();
        let _ = text.parse::<Channel>();
        let _ = Manifest::parse(&text);
        let _ = mtree::parse(&text, Path::new("mtree.txt.gz"));
        let _ = HostLibs::parse(&text);
        #[cfg(feature = "download")]
        {
            let _ = libsruman::sync::parse_sha256sums(&text);
            let _ = libsruman::sync::parse_os_release(&text);
            let _ = libsruman::sync::parse_autoindex(&text, None);
            let _ = libsruman::sync::parse_autoindex(&text, Some(3));
            let _ = libsruman::fetch::parse_content_range(&text);
        }
    }

    #[test]
    fn build_ids_round_trip_and_order_totally(a in build_id(), b in build_id(), c in build_id()) {
        prop_assert_eq!(a.to_string().parse::<BuildId>().unwrap(), a.clone());
        // Antisymmetric, consistent with Eq, transitive.
        prop_assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
        prop_assert_eq!(a.cmp(&b) == Ordering::Equal, a == b);
        if a <= b && b <= c {
            prop_assert!(a <= c);
        }
        // Numeric, part by part.
        prop_assert_eq!(a.parts().cmp(b.parts()) == Ordering::Less, a < b && a.parts() != b.parts());
    }

    #[test]
    fn runtime_refs_round_trip(
        name in "[A-Za-z0-9_][A-Za-z0-9._-]{0,20}",
        build in prop::option::of(build_id()),
    ) {
        prop_assume!(name != "." && name != "..");
        let r = RuntimeRef::new(name, build).unwrap();
        prop_assert_eq!(r.to_string().parse::<RuntimeRef>().unwrap(), r);
    }

    /// Applying `old.diff(new)` to `old` gives `new` (keys and versions).
    #[test]
    fn manifest_diff_applied_gives_the_new_manifest(
        old in packages(),
        new in packages(),
    ) {
        let (old_m, new_m) = (manifest(&old), manifest(&new));
        let diff = old_m.diff(&new_m);
        let mut applied: BTreeMap<String, String> = old.clone();
        for p in &diff.removed {
            applied.remove(&p.key());
        }
        for c in &diff.changed {
            let key = match &c.arch {
                Some(arch) => format!("{}:{arch}", c.name),
                None => c.name.clone(),
            };
            prop_assert_eq!(applied.get(&key), Some(&c.old));
            applied.insert(key, c.new.clone());
        }
        for p in &diff.added {
            prop_assert!(applied.insert(p.key(), p.version.clone()).is_none());
        }
        prop_assert_eq!(applied, new);
        prop_assert_eq!(old_m.diff(&old_m).is_empty(), true);
    }
}

/// `key → version`, keys as `Package::key` writes them.
fn packages() -> impl Strategy<Value = BTreeMap<String, String>> {
    prop::collection::btree_map(
        (
            "[a-d]{1,2}",
            prop::option::of(prop::sample::select(vec!["amd64", "i386"])),
        )
            .prop_map(|(name, arch)| match arch {
                Some(arch) => format!("{name}:{arch}"),
                None => name,
            }),
        "[0-9][0-9a-z.+~-]{0,8}",
        0..12,
    )
}

fn manifest(packages: &BTreeMap<String, String>) -> Manifest {
    let mut text = String::from("#Package[:Architecture]\t#Version\t#Source\t#Installed-Size\n");
    for (key, version) in packages {
        text.push_str(&format!("{key}\t{version}\t\t1\n"));
    }
    Manifest::parse(&text).unwrap()
}

#[cfg(feature = "download")]
mod archives {
    use libsruman::{
        install::extract::{Limits, extract_tar},
        progress::CancelFlag,
    };
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Arbitrary bytes, and real archives with bytes flipped: an error
        /// or a tree inside `dest`, never a panic or a write outside.
        #[test]
        fn the_extractor_takes_any_bytes(
            noise in prop::collection::vec(any::<u8>(), 0..4096),
            flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
        ) {
            let mut archive = sample_tar();
            for (at, byte) in flips {
                let i = at.index(archive.len());
                archive[i] ^= byte;
            }
            for input in [&noise[..], &archive[..]] {
                let tmp = tempfile::tempdir().unwrap();
                let dest = tmp.path().join("dest");
                std::fs::create_dir(&dest).unwrap();
                let limits = Limits { bytes: 1 << 20, entries: 1000 };
                let _ = extract_tar(input, &dest, limits, &CancelFlag::new(), &mut || {});
                let outside: Vec<_> = std::fs::read_dir(tmp.path())
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                prop_assert_eq!(outside, ["dest"]);
            }
        }
    }

    fn sample_tar() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut add = |path: &str, kind: tar::EntryType, data: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(0o755);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
        };
        add("top/", tar::EntryType::Directory, b"");
        add("top/files/", tar::EntryType::Directory, b"");
        add("top/files/a.txt", tar::EntryType::Regular, b"hello");
        add("top/run", tar::EntryType::Regular, b"#!/bin/sh\n");
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        builder
            .append_link(&mut link, "top/l", "files/a.txt")
            .unwrap();
        builder.into_inner().unwrap()
    }
}
