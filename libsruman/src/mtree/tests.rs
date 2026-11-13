use std::{fs, io::Write, os::unix::fs::symlink};

use super::*;

const SHA_HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn sample() -> String {
    format!(
        "#mtree\n\
         . type=dir\n\
         ./run type=file mode=755 size=5 sha256={SHA_HELLO}\n\
         ./a\\040b\\176c type=file size=0\n\
         ./bin type=dir\n\
         ./bin/tool type=link link=../run\n\
         ./var type=dir ignore optional\n\
         ./lib/x.so type=file size=5 sha256={SHA_HELLO} contents=./53/obj-1.bin\n\
         ./lib/y.so type=file size=5 sha256={SHA_HELLO} contents=./53/obj-1.bin\n"
    )
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("run"), "hello").unwrap();
    fs::write(root.join("a b~c"), "").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    symlink("../run", root.join("bin/tool")).unwrap();
    fs::create_dir(root.join("53")).unwrap();
    fs::write(root.join("53/obj-1.bin"), "hello").unwrap();
    dir
}

#[test]
fn parses_entries_and_escapes() {
    let entries = parse(&sample(), Path::new("t")).unwrap();
    assert_eq!(entries.len(), 8);
    assert_eq!(entries[0].path, Path::new("."));
    assert_eq!(entries[1].kind, Kind::File);
    assert_eq!(entries[1].size, Some(5));
    assert_eq!(entries[2].path, Path::new("a b~c"));
    assert_eq!(entries[4].link.as_deref(), Some(Path::new("../run")));
    assert!(entries[5].optional);
    assert_eq!(entries[6].data_path(), Path::new("53/obj-1.bin"));
}

#[test]
fn rejects_bad_lines() {
    for (line, expected) in [
        ("./x size=1", "missing `type=`"),
        ("./x type=fifo", "unsupported type"),
        ("./x type=link", "link without"),
        ("./x type=file size=big", "bad size"),
        ("./x type=file sha256=abc", "bad sha256"),
        ("../x type=file", "leaves the tree"),
        ("./a/../../x type=file", "leaves the tree"),
        ("./x type=file contents=../y", "leaves the tree"),
        ("/set type=file", "not supported"),
        ("./x\\9 type=file", "bad escape"),
    ] {
        let err = parse(line, Path::new("m")).unwrap_err().to_string();
        assert!(err.contains(expected), "{line}: {err}");
        assert!(err.starts_with("m line 1:"), "{err}");
    }
}

#[test]
fn verify_clean_tree() {
    let dir = tree();
    let entries = parse(&sample(), Path::new("t")).unwrap();
    let report = verify(dir.path(), &entries, Depth::Content, false);
    assert_eq!(report.problems, []);
    // `var` is optional and the shared object is checked once.
    assert_eq!(report.checked, 6);
}

#[test]
fn verify_finds_each_kind_of_problem() {
    let dir = tree();
    let root = dir.path();
    fs::remove_file(root.join("a b~c")).unwrap();
    fs::write(root.join("run"), "HELLO").unwrap(); // same size, different content
    fs::remove_file(root.join("bin/tool")).unwrap();
    symlink("elsewhere", root.join("bin/tool")).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(root.join("53/obj-1.bin"))
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let entries = parse(&sample(), Path::new("t")).unwrap();

    let presence = verify(root, &entries, Depth::Presence, false);
    let text: Vec<String> = presence.problems.iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        [
            "a b~c: missing",
            "bin/tool: links to elsewhere, expected ../run",
            "lib/x.so: size 6, expected 5",
        ]
    );

    let content = verify(root, &entries, Depth::Content, false);
    assert!(content.problems.contains(&Problem::WrongHash("run".into())));
}

#[test]
fn objects_only_skips_dirs_and_links() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("53")).unwrap();
    fs::write(dir.path().join("53/obj-1.bin"), "hello").unwrap();
    let entries = parse(&sample(), Path::new("t")).unwrap();
    let report = verify(dir.path(), &entries, Depth::Content, true);
    let text: Vec<String> = report.problems.iter().map(ToString::to_string).collect();
    // The empty `a b~c` has no object to check.
    assert_eq!(text, ["run: missing"]);
}

#[test]
fn reads_gzip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mtree.txt.gz");
    let mut gz = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::fast(),
    );
    gz.write_all(sample().as_bytes()).unwrap();
    gz.finish().unwrap();
    assert_eq!(read_gz(&path).unwrap().len(), 8);
    assert!(matches!(
        read_gz(&dir.path().join("none")),
        Err(MtreeError::Io { .. })
    ));
}

#[test]
fn hex_and_file_hash() {
    assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
    let dir = tree();
    assert_eq!(sha256_file(&dir.path().join("run")).unwrap(), SHA_HELLO);
}

/// The real file lists of Steam's own runtime installs, when present.
#[test]
#[ignore = "needs Steam's SteamLinuxRuntime_soldier installed"]
fn real_steam_soldier_verifies() {
    let home = std::env::var_os("HOME").unwrap();
    let root =
        Path::new(&home).join(".local/share/Steam/steamapps/common/SteamLinuxRuntime_soldier");
    let top = read_gz(&root.join("mtree.txt.gz")).unwrap();
    assert_eq!(verify(&root, &top, Depth::Content, false).problems, []);
    let platform = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("soldier_platform_")
        })
        .unwrap();
    let usr = read_gz(&platform.join("usr-mtree.txt.gz")).unwrap();
    let report = verify(&platform.join("files"), &usr, Depth::Presence, true);
    assert_eq!(report.problems, []);
    assert!(report.checked > 1000);
}
