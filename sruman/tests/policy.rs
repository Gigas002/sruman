//! Source policies from the audit (PLAN 5.5a) that clippy can't express.

use std::{fs, path::Path};

/// Every non-test `.rs` file of both crates.
fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut dirs = vec![root.join("libsruman/src"), root.join("sruman/src")];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                dirs.push(path);
            } else if name.ends_with(".rs") && name != "tests.rs" && name != "testutil.rs" {
                out.push((
                    path.display().to_string(),
                    fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    assert!(out.len() > 20, "found only {} source files", out.len());
    out
}

/// Commands are argv vectors, never a string for `sh -c` to parse (the
/// interactive `sruman shell` starts a shell without `-c`).
#[test]
fn nothing_runs_through_a_shell() {
    for (path, text) in sources() {
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            assert!(
                !code.contains("\"-c\""),
                "{path}:{}: `-c` looks like a shell command line; run the program directly",
                n + 1
            );
        }
    }
}

#[test]
fn no_unsafe_code() {
    for (path, text) in sources() {
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            assert!(
                !code.contains("unsafe ") && !code.contains("unsafe{"),
                "{path}:{}: unsafe code",
                n + 1
            );
        }
    }
}
