use std::{
    collections::HashMap, ffi::OsString, fs, os::unix::process::ExitStatusExt, path::Path,
    process::ExitStatus, time::Duration,
};

use clap::Parser;

use super::*;
use crate::{
    cli::Cli,
    settings::{CliOptions, Environment},
};

fn settings(home: &Path, args: &[&str]) -> Settings {
    let cli = Cli::try_parse_from(std::iter::once("sruman").chain(args.iter().copied())).unwrap();
    let vars: HashMap<&str, OsString> = HashMap::from([("HOME", home.into())]);
    let env = Environment {
        lookup: move |key: &str| vars.get(key).cloned(),
        cwd: home.to_path_buf(),
    };
    Settings::resolve(CliOptions::from(cli), env).unwrap()
}

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn supervised_args_repeat_the_run() {
    let home = tempfile::tempdir().unwrap();
    let s = settings(
        home.path(),
        &[
            "-vv", "run", "game", "-p", "box", "--debug", "--", "-x", "y z",
        ],
    );
    assert_eq!(
        strings(&supervised_args(&s)),
        [
            "-vv",
            "run",
            "game",
            "--profile",
            "box",
            "--supervised",
            "--debug",
            "--",
            "-x",
            "y z"
        ]
    );
    // No extra args, no `--`; the config is passed on once it exists.
    let config = home.path().join(".config/sruman/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "").unwrap();
    let s = settings(home.path(), &["-v", "run", "game"]);
    assert_eq!(
        strings(&supervised_args(&s)),
        [
            "--config",
            config.to_str().unwrap(),
            "-v",
            "run",
            "game",
            "--supervised"
        ]
    );
}

#[test]
fn supervised_args_repeat_the_execute() {
    let home = tempfile::tempdir().unwrap();
    let s = settings(
        home.path(),
        &["execute", "-p", "box", "bin/tool", "--", "a"],
    );
    let target = home.path().join("bin/tool");
    assert_eq!(
        strings(&supervised_args(&s)),
        [
            "execute",
            "--profile",
            "box",
            target.to_str().unwrap(),
            "--supervised",
            "--",
            "a"
        ]
    );
}

#[test]
fn read_status_reports() {
    let started = read_status("noise\nok\t42\t/logs/x.log\n".as_bytes()).unwrap();
    assert_eq!(
        (started.pid, started.log),
        (42, PathBuf::from("/logs/x.log"))
    );

    let err = read_status("error\t4\n`game` is already running\nmore\n".as_bytes()).unwrap_err();
    assert_eq!(err.exit_code(), 4);
    assert_eq!(err.to_string(), "`game` is already running\nmore");

    let err = read_status("".as_bytes()).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert!(err.to_string().contains("without reporting"));
}

#[test]
fn exits_and_footers() {
    let code = |c: i32| ExitStatus::from_raw(c << 8);
    let signal = ExitStatus::from_raw;
    assert_eq!(exit_code(code(0)), 0);
    assert_eq!(exit_code(code(7)), 7);
    assert_eq!(exit_code(signal(15)), 143);
    assert_eq!(
        footer(code(3), Duration::from_secs(3725)),
        "=== exit: code 3 — duration 01:02:05 ===\n"
    );
    assert_eq!(
        footer(signal(9), Duration::from_millis(900)),
        "=== exit: signal 9 — duration 00:00:00 ===\n"
    );
}

#[test]
fn command_lines_are_shell_quoted() {
    let argv = ["/opt/game/run", "--x=1", "", "two words", "it's", "a$b"].map(std::ffi::OsStr::new);
    assert_eq!(
        command_line(argv.into_iter()),
        r#"/opt/game/run --x=1 '' 'two words' 'it'\''s' 'a$b'"#
    );
}
