//! Docs and code agree (PLAN 5.5c): every command in clap has a row in
//! `docs/USAGE.md` and every row is a command; every option is mentioned;
//! `examples/config.toml` holds every key `config.toml` takes, and works,
//! commented-out keys included.

#![allow(clippy::disallowed_methods, reason = "tests start the sruman binary")]

use std::{fs, path::Path, process::Command};

use sruman::config::FileConfig;
#[cfg(all(feature = "download", feature = "json", feature = "diag"))]
use {clap::CommandFactory, sruman::cli::Cli, std::collections::BTreeSet};

fn repo(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)).unwrap()
}

/// `sync`, `profiles list`, … as clap knows them (help excluded).
#[cfg(all(feature = "download", feature = "json", feature = "diag"))]
fn clap_commands() -> BTreeSet<String> {
    let cli = Cli::command();
    let mut out = BTreeSet::new();
    for sub in cli.get_subcommands().filter(|c| c.get_name() != "help") {
        let nested: Vec<_> = sub
            .get_subcommands()
            .filter(|c| c.get_name() != "help")
            .collect();
        if nested.is_empty() {
            out.insert(sub.get_name().to_owned());
        }
        for n in nested {
            out.insert(format!("{} {}", sub.get_name(), n.get_name()));
        }
    }
    out
}

/// The `` | `sruman <command>` | `` rows of USAGE.md's tables.
#[cfg(all(feature = "download", feature = "json", feature = "diag"))]
fn documented_commands(usage: &str) -> BTreeSet<String> {
    usage
        .lines()
        .filter_map(|line| line.strip_prefix("| `sruman "))
        .filter_map(|rest| rest.split_once("` |"))
        .map(|(command, _)| command.to_owned())
        .collect()
}

#[test]
#[cfg(all(feature = "download", feature = "json", feature = "diag"))]
fn usage_lists_exactly_the_commands() {
    let usage = repo("docs/USAGE.md");
    let (code, docs) = (clap_commands(), documented_commands(&usage));
    let undocumented: Vec<_> = code.difference(&docs).collect();
    let stale: Vec<_> = docs.difference(&code).collect();
    assert!(
        undocumented.is_empty() && stale.is_empty(),
        "not in docs/USAGE.md: {undocumented:?}; not commands: {stale:?}"
    );
}

#[test]
#[cfg(all(feature = "download", feature = "json", feature = "diag"))]
fn usage_mentions_every_option() {
    let usage = repo("docs/USAGE.md");
    let mut missing = Vec::new();
    let mut visit = |command: &clap::Command, path: &str| {
        for arg in command.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            if arg.is_hide_set() || ["help", "version"].contains(&long) {
                continue;
            }
            let short = arg
                .get_short()
                .map(|s| format!("`-{s}"))
                .unwrap_or_default();
            let short_used = !short.is_empty()
                && (usage.contains(&format!("{short}`")) || usage.contains(&format!("{short} ")));
            if !usage.contains(&format!("--{long}")) && !short_used {
                missing.push(format!("{path} --{long}"));
            }
        }
    };
    let cli = Cli::command();
    visit(&cli, "sruman");
    for sub in cli.get_subcommands() {
        visit(sub, sub.get_name());
        for nested in sub.get_subcommands() {
            visit(nested, &format!("{} {}", sub.get_name(), nested.get_name()));
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "options not in docs/USAGE.md: {missing:?}"
    );
}

/// Every key `config.toml` takes: serde lists them when it meets an
/// unknown one.
fn config_keys() -> Vec<String> {
    let err = toml::from_str::<FileConfig>("not_a_key = 1")
        .unwrap_err()
        .to_string();
    let list = err.split("expected one of").nth(1).expect(&err);
    list.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The example with every `# key = …`, `# [table]` and following `# k = v`
/// lines enabled.
fn uncommented(example: &str) -> String {
    example
        .lines()
        .map(|line| {
            let rest = line.strip_prefix("# ").unwrap_or(line);
            let looks_like_toml = rest.starts_with('[')
                || rest.split_once(" = ").is_some_and(|(key, _)| {
                    key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
            if line.starts_with("# ") && looks_like_toml {
                rest.to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_example_config_has_every_key() {
    let full = uncommented(&repo("examples/config.toml"));
    let value: toml::Value = toml::from_str(&full).unwrap();
    let table = value.as_table().unwrap();
    // Read only to warn that it does nothing now (it moved to gaki).
    for key in config_keys()
        .into_iter()
        .filter(|key| key != "steam_library")
    {
        assert!(
            table.contains_key(&key),
            "examples/config.toml lacks `{key}`"
        );
    }
    toml::from_str::<FileConfig>(&full).unwrap();
}

#[test]
fn the_example_config_works_as_shipped_and_uncommented() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    for (name, text) in [
        ("shipped.toml", repo("examples/config.toml")),
        ("full.toml", uncommented(&repo("examples/config.toml"))),
    ] {
        let path = tmp.path().join(name);
        fs::write(&path, text).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_sruman"))
            .args(["--config", path.to_str().unwrap(), "runtimes"])
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", tmp.path().join("config"))
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_CACHE_HOME", tmp.path().join("cache"))
            .env("XDG_STATE_HOME", tmp.path().join("state"))
            .env("XDG_RUNTIME_DIR", tmp.path().join("run"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
