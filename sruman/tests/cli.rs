//! The `sruman` binary end to end (4.6), in a private set of XDG
//! directories: the parse matrix's exit codes, `check` on the examples,
//! `run`/`execute` in both modes against a fake runtime whose `run` records
//! its argv and env, and `install`/`upgrade --sync` against libsruman's
//! fixture image server.

#![allow(
    clippy::disallowed_methods,
    reason = "tests start fake runtimes and helper processes"
)]

#[cfg(feature = "download")]
#[path = "../../libsruman/tests/support/http.rs"]
mod http;
#[cfg(feature = "download")]
#[path = "../../libsruman/tests/support/images.rs"]
mod images;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// A home and XDG directories of its own.
struct Sruman {
    tmp: tempfile::TempDir,
}

impl Sruman {
    fn new() -> Self {
        let sruman = Self {
            tmp: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(sruman.config()).unwrap();
        fs::create_dir_all(sruman.home()).unwrap();
        sruman
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    fn config(&self) -> PathBuf {
        self.root().join("config/sruman")
    }

    fn logs(&self) -> PathBuf {
        self.root().join("state/sruman/logs")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sruman"));
        command
            .args(args)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_RUNTIME_DIR", self.root().join("run"))
            .env("NO_COLOR", "1")
            .env("SRUMAN_TEST_OUT", self.root().join("runtime.out"))
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn write_config(&self, file: &str, text: &str) {
        let path = self.config().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A container runtime directory (`steamrt4` 4.0.1) whose `run`
    /// writes its argv and the runtime env to `$SRUMAN_TEST_OUT`, then runs
    /// the command after `--`.
    fn fake_runtime(&self) -> PathBuf {
        let dir = self.root().join("SteamLinuxRuntime_4");
        fs::create_dir_all(dir.join("steamrt4_platform_4.0.1/files")).unwrap();
        fs::write(dir.join("VERSIONS.txt"), "#Name\tVersion\ndepot\t4.0.1\n").unwrap();
        fs::write(dir.join("_v2-entry-point"), "#!/bin/sh\n").unwrap();
        script(
            &dir.join("run"),
            r#"{ printf 'argv:'; for a in "$@"; do printf ' [%s]' "$a"; done; echo
env | grep -E '^(PRESSURE_VESSEL|SRUMAN_)' | sort; } > "$SRUMAN_TEST_OUT"
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
exec "$@""#,
        );
        dir
    }

    /// A game executable that prints its arguments and home, sleeps
    /// `$GAME_SLEEP` and exits with `$GAME_EXIT`.
    fn game(&self) -> PathBuf {
        let path = self.home().join("Games/game.sh");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        script(
            &path,
            r#"echo "game: $* HOME=$HOME LD=$LD_LIBRARY_PATH"
echo "complaint" >&2
sleep "${GAME_SLEEP:-0}"
exit "${GAME_EXIT:-0}""#,
        );
        path
    }

    fn runtime_out(&self) -> String {
        fs::read_to_string(self.root().join("runtime.out")).unwrap_or_default()
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[track_caller]
fn expect(output: &Output, code: i32) -> (String, String) {
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

/// Waits up to 10 s for `done`.
fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Every exit code (0 ok, 1 failed, 2 invalid, 3 not running, 4 busy), and
/// a snapshot of what each failure prints (PLAN 5.5c): every error should
/// name a fix.
#[test]
fn parse_matrix_exit_codes() {
    let s = Sruman::new();
    let root = s.root().display().to_string();
    let mut shown = String::new();
    let mut case = |args: &[&str], code: i32| -> String {
        let (_, err) = expect(&s.run(args), code);
        shown.push_str(&format!(
            "$ sruman {} -> {code}\n{}\n",
            args.join(" "),
            err.replace(&root, "<tmp>").trim_end()
        ));
        err
    };

    let help = case(&[], 2);
    assert!(help.contains("Usage: sruman [OPTIONS] <COMMAND>"), "{help}");
    for args in [
        &["frobnicate"][..],
        &["-S", "sniper"],
        &["remove", "sniper=not-a-build"],
        &["run", "./path"],
        &["runtimes", "--wait"],
        &["stop"],
    ] {
        case(args, 2);
    }
    // Valid to clap, but no profile to run with.
    let err = case(&["shell"], 2);
    assert!(err.contains("default_profile"), "{err}");
    let err = case(&["run", "nothing"], 2);
    assert!(err.contains("no game named `nothing`"), "{err}");
    let err = case(&["stop", "nothing"], 3);
    assert!(err.contains("`nothing` is not running"), "{err}");
    let err = case(&["logs", "nothing"], 3);
    assert!(err.contains("no run logs"), "{err}");
    let err = case(&["import", "/nonexistent"], 1);
    assert!(err.contains("/nonexistent: No such file"), "{err}");
    // The store is locked by someone else.
    {
        let store = libsruman::store::Store::open(s.root().join("data/sruman/runtimes"));
        let _lock = store.lock().unwrap();
        let err = case(&["import", "/nonexistent"], 4);
        assert!(err.contains("busy") || err.contains("lock"), "{err}");
    }
    // A broken config is exit 2 before any command runs.
    s.write_config("config.toml", "log_keep = 0\n");
    let err = case(&["runtimes"], 2);
    assert!(err.contains("`log_keep`"), "{err}");
    drop(case);

    let (version, _) = expect(&s.run(&["--version"]), 0);
    assert!(version.starts_with("sruman "), "{version}");
    // The command list in the help differs with the features.
    if cfg!(all(
        feature = "download",
        feature = "json",
        feature = "diag"
    )) {
        insta::assert_snapshot!("errors_full", shown);
    } else if cfg!(not(any(
        feature = "download",
        feature = "json",
        feature = "diag"
    ))) {
        insta::assert_snapshot!("errors_minimal", shown);
    }
}

#[test]
fn check_on_the_examples() {
    let s = Sruman::new();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    fs::create_dir_all(s.config().join("profiles")).unwrap();
    for entry in fs::read_dir(examples.join("profiles")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(
            entry.path(),
            s.config().join("profiles").join(entry.file_name()),
        )
        .unwrap();
    }
    fs::copy(examples.join("games.toml"), s.config().join("games.toml")).unwrap();
    s.write_config("config.toml", "default_profile = \"sniper\"\n");

    // Nothing is installed here, and wrappers may be missing; the files
    // themselves are fine.
    let (_, err) = expect(&s.run(&["check"]), 2);
    let errors: Vec<&str> = err.lines().filter(|l| l.starts_with("error:")).collect();
    assert!(!errors.is_empty(), "{err}");
    for error in errors {
        assert!(
            error.contains("is not installed") || error.contains("not found in PATH"),
            "{error}"
        );
    }
    let (list, _) = expect(&s.run(&["list"]), 0);
    assert!(list.contains("hollow-knight"), "{list}");
    let (profiles, _) = expect(&s.run(&["profiles", "list"]), 0);
    assert!(profiles.contains("shared-prefix-a"), "{profiles}");
}

/// The fake steamrt4 imported, a container profile with a prefix, and a
/// game on it.
fn container_world(game: &str) -> (Sruman, PathBuf) {
    let s = Sruman::new();
    let runtime = s.fake_runtime();
    let (out, _) = expect(&s.run(&["import", runtime.to_str().unwrap()]), 0);
    assert!(out.contains("imported steamrt4 4.0.1"), "{out}");
    let (out, _) = expect(
        &s.run(&[
            "profiles",
            "new",
            "box",
            "--runtime",
            "steamrt4",
            "--prefix",
            "~/pfx",
        ]),
        0,
    );
    assert!(out.contains("pfx: created"), "{out}");
    let exe = s.game();
    s.write_config(
        "games.toml",
        &format!(
            "[[game]]\nname = \"{game}\"\nexe = \"{}\"\nprofile = \"box\"\nargs = [\"--from-games\"]\n",
            exe.display()
        ),
    );
    (s, exe)
}

/// Masks what changes from run to run: times, and live state that other
/// tests running at the same time can affect (their games show up in
/// `/proc`, and one of them uses a profile called `box` too).
#[cfg(feature = "json")]
fn stable(value: &mut serde_json::Value, root: &str) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if ["installed_at", "in_use", "running"].contains(&key.as_str()) {
                    *v = Value::String("<live>".into());
                } else {
                    stable(v, root);
                }
            }
        }
        Value::Array(items) => {
            // Sessions of other tests' worlds.
            items.retain(|item| {
                item.get("prefix")
                    .and_then(Value::as_str)
                    .is_none_or(|p| p.starts_with(root) || item.get("pid").is_none())
            });
            for item in items {
                stable(item, root);
            }
        }
        _ => {}
    }
}

/// The `--json` output of every command that doesn't need the network, as
/// snapshots (PLAN 5.5c): a schema change shows up as a diff. Temporary
/// paths are masked.
#[cfg(feature = "json")]
#[test]
fn json_schemas() {
    let (s, _) = container_world("cli-json");
    let root = s.root().display().to_string();
    let mut shown = String::new();
    for args in [
        &["runtimes", "--json"][..],
        &["info", "steamrt4", "--json"],
        &["list", "--json"],
        &["show", "cli-json", "--json"],
        &["profiles", "list", "--json"],
        &["profiles", "show", "box", "--json"],
        &["status", "--json"],
    ] {
        let (out, _) = expect(&s.run(args), 0);
        let mut value: serde_json::Value = serde_json::from_str(&out).expect(&out);
        stable(&mut value, &root);
        let text = serde_json::to_string_pretty(&value).unwrap();
        shown.push_str(&format!(
            "$ sruman {}\n{}\n",
            args.join(" "),
            text.replace(&root, "<tmp>")
        ));
    }
    insta::assert_snapshot!("json", shown);
}

#[test]
fn run_and_execute_in_container_mode() {
    let (s, exe) = container_world("cli-container");
    let (_, err) = expect(&s.run(&["check"]), 0);
    assert!(!err.contains("error"), "{err}");

    // --wait: the game's output and exit code come back.
    let output = s
        .command(&["run", "cli-container", "--wait", "--", "extra", "--x"])
        .env("GAME_EXIT", "5")
        .output()
        .unwrap();
    let (stdout, stderr) = expect(&output, 5);
    assert!(stdout.contains("game: --from-games extra --x"), "{stdout}");
    assert!(stderr.contains("complaint"), "{stderr}");
    let recorded = s.runtime_out();
    let argv = recorded.lines().next().unwrap();
    assert!(argv.contains("[--pass-fd="), "{argv}");
    assert!(
        argv.ends_with(&format!(
            "[--] [{}] [--from-games] [extra] [--x]",
            exe.display()
        )),
        "{argv}"
    );
    let prefix_home = s.home().join("pfx/home");
    assert!(
        recorded.contains(&format!("PRESSURE_VESSEL_HOME={}", prefix_home.display())),
        "{recorded}"
    );
    assert!(recorded.contains("SRUMAN_GAME=cli-container"), "{recorded}");
    assert!(recorded.contains("SRUMAN_PROFILE=box"), "{recorded}");
    assert!(prefix_home.join(".config").is_dir());

    // The run log: header, stamped output, footer; `logs` finds it.
    let (log, _) = expect(&s.run(&["logs", "cli-container"]), 0);
    assert!(log.starts_with("=== sruman "), "{log}");
    assert!(log.contains("--- command ---"), "{log}");
    assert!(
        log.contains("[stdout] game: --from-games extra --x"),
        "{log}"
    );
    assert!(log.contains("[stderr] complaint"), "{log}");
    assert!(
        log.trim_end()
            .ends_with("=== exit: code 5 — duration 00:00:00 ==="),
        "{log}"
    );
    let (path, _) = expect(&s.run(&["logs", "cli-container", "--path"]), 0);
    assert!(
        path.trim()
            .starts_with(s.logs().join("games/cli-container").to_str().unwrap())
    );

    // `show` previews the same command.
    let (show, _) = expect(&s.run(&["show", "cli-container"]), 0);
    assert!(show.contains("Runtime     : steamrt4 4.0.1"), "{show}");
    assert!(show.contains("Running     : No"), "{show}");

    // `execute` logs under exec/<profile>.
    let (stdout, _) = expect(
        &s.run(&[
            "execute",
            "-p",
            "box",
            exe.to_str().unwrap(),
            "--wait",
            "--",
            "one",
        ]),
        0,
    );
    assert!(stdout.contains("game: one"), "{stdout}");
    assert!(s.logs().join("exec/box/latest.log").is_file());
}

#[test]
fn detached_runs_status_and_stop() {
    let (s, _) = container_world("cli-detached");
    let start = |sleep: &str| {
        s.command(&["run", "cli-detached"])
            .env("GAME_SLEEP", sleep)
            .output()
            .unwrap()
    };
    let (out, _) = expect(&start("30"), 0);
    assert!(out.starts_with("started `cli-detached` (pid "), "{out}");
    let log = PathBuf::from(out.lines().nth(1).unwrap().trim_start_matches("log: "));
    assert!(log.is_file(), "{out}");

    let (status, _) = expect(&s.run(&["status"]), 0);
    assert!(status.starts_with("cli-detached  pid "), "{status}");
    let (list, _) = expect(&s.run(&["list"]), 0);
    assert!(list.contains("running"), "{list}");
    // One instance per game.
    let (_, err) = expect(&start("0"), 4);
    assert!(err.contains("already running"), "{err}");
    // The build is held while it runs.
    let (_, err) = expect(&s.run(&["-y", "remove", "--force", "steamrt4"]), 4);
    assert!(err.contains("in use"), "{err}");

    let (out, _) = expect(&s.run(&["stop", "cli-detached"]), 0);
    assert_eq!(out, "stopped cli-detached\n");
    // The supervisor outlives the game and writes the footer.
    assert!(
        wait_for(|| fs::read_to_string(&log).is_ok_and(|t| t.contains("=== exit: signal 15"))),
        "{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
    expect(&s.run(&["stop", "cli-detached"]), 3);

    // A detached run that finishes on its own.
    let (out, _) = expect(&start("0"), 0);
    let log = PathBuf::from(out.lines().nth(1).unwrap().trim_start_matches("log: "));
    assert!(
        wait_for(|| fs::read_to_string(&log).is_ok_and(|t| t.contains("=== exit: code 0"))),
        "{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
}

#[test]
fn execute_in_overlay_mode() {
    let s = Sruman::new();
    let runtime = s.fake_runtime();
    expect(&s.run(&["import", "--link", runtime.to_str().unwrap()]), 0);
    let (out, _) = expect(
        &s.run(&[
            "profiles", "new", "host", "-r", "steamrt4", "--mode", "overlay",
        ]),
        0,
    );
    assert!(!out.contains("prefix"), "no prefix without --prefix: {out}");
    let exe = s.game();
    let (stdout, _) = expect(
        &s.run(&["execute", "-p", "host", exe.to_str().unwrap(), "--wait"]),
        0,
    );
    // On the host, without the runtime's `run`, in the real home, with
    // the overlay folder on LD_LIBRARY_PATH.
    assert!(
        stdout.contains(&format!("HOME={}", s.home().display())),
        "{stdout}"
    );
    assert!(stdout.contains(".overlays/steamrt4/4.0.1/"), "{stdout}");
    assert_eq!(s.runtime_out(), "");
    // Removing a linked build only unregisters it.
    expect(&s.run(&["-y", "remove", "--force", "steamrt4"]), 0);
    assert!(runtime.join("run").is_file());
}

#[test]
fn provide_umu_builds_the_view() {
    let s = Sruman::new();
    let (_, err) = expect(&s.run(&["provide-umu", "--appid", "4183110"]), 2);
    assert!(err.contains("sruman install steamrt4"), "{err}");
    let runtime = s.fake_runtime();
    expect(&s.run(&["import", "--link", runtime.to_str().unwrap()]), 0);

    let (out, _) = expect(&s.run(&["provide-umu", "--appid", "4183110"]), 0);
    let folders = s.root().join("data/sruman");
    assert_eq!(
        out,
        format!(
            "steamrt4 4.0.1 for umu: provided\nUMU_FOLDERS_PATH={}\nhold: {}\n",
            folders.display(),
            folders.join("runtimes/steamrt4/4.0.1.lock").display()
        )
    );
    let view = folders.join("umu/steamrt4");
    assert_eq!(
        fs::read_to_string(view.join(".installed.ok")).unwrap(),
        "ok\n"
    );
    assert_eq!(
        fs::read_link(view.join("umu")).unwrap(),
        Path::new("_v2-entry-point")
    );
    assert!(view.join("run").is_file());
    let (out, _) = expect(&s.run(&["provide-umu", "steamrt4"]), 0);
    assert!(
        out.starts_with("steamrt4 4.0.1 for umu: up to date\n"),
        "{out}"
    );
    let (_, err) = expect(&s.run(&["provide-umu", "scout"]), 2);
    assert!(err.contains("umu has no folder for scout"), "{err}");
}

#[cfg(feature = "download")]
mod server {
    use super::*;
    use crate::{http::Server, images::Suite};

    const OLD: &str = "3.0.20261021.266025";
    const NEW: &str = "3.0.20261110.268881";

    fn manifest(build: &str) -> String {
        fs::read_to_string(format!(
            "{}/../libsruman/tests/fixtures/manifests/{build}.sniper.manifest.dpkg",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn install_and_upgrade_over_http() {
        let server = Server::start();
        let sniper = Suite {
            server: &server,
            dir: "steamrt3",
            suite: "sniper",
            archive: "SteamLinuxRuntime_sniper.tar.xz",
        };
        sniper.publish(OLD, &manifest(OLD));
        sniper.publish(NEW, &manifest(NEW));
        sniper.point("latest-public-stable", OLD, "t1");

        let s = Sruman::new();
        let mut config = String::new();
        for name in ["scout", "soldier", "sniper", "steamrt4"] {
            config += &format!(
                "[[runtime]]\nname = \"{name}\"\nbase_url = \"{}\"\n",
                server.base
            );
        }
        s.write_config("config.toml", &config);

        // The others aren't published: refreshing them fails, sniper works.
        let (out, err) = expect(&s.run(&["sync"]), 1);
        assert!(out.contains(&format!(" sniper {OLD} (stable)")), "{out}");
        assert!(err.contains("failed to synchronize soldier"), "{err}");

        let (out, err) = expect(&s.run(&["install", "sniper"]), 0);
        assert!(
            out.contains(&format!("Runtimes (1)  sniper-{OLD}")),
            "{out}"
        );
        assert!(out.contains("Proceed with installation? [Y/n]"), "{out}");
        assert!(err.contains(&format!("downloading sniper {OLD}")), "{err}");
        let (out, _) = expect(&s.run(&["runtimes"]), 0);
        assert_eq!(out, format!("sniper {OLD}\n"));
        let (out, _) = expect(&s.run(&["outdated"]), 0);
        assert_eq!(out, "");

        sniper.point("latest-public-stable", NEW, "t2");
        let (out, _) = expect(&s.run(&["upgrade", "--sync"]), 1);
        assert!(out.contains("Package changes in sniper:"), "{out}");
        assert!(
            out.contains(&format!("Runtimes (1)  sniper-{NEW}")),
            "{out}"
        );
        let (out, _) = expect(&s.run(&["runtimes", "--json"]), 0);
        assert!(
            out.contains(&format!("\"build\": \"{NEW}\",\n    \"current\": true")),
            "{out}"
        );
        let (out, _) = expect(&s.run(&["info", "sniper"]), 0);
        assert!(out.contains(&format!("Build        : {NEW}")), "{out}");

        // keep_versions = 0: `clean` drops the old build and its archive.
        s.write_config("config.toml", &format!("keep_versions = 0\n{config}"));
        let (out, _) = expect(&s.run(&["clean"]), 0);
        assert!(out.contains(&format!("removed sniper {OLD}")), "{out}");
        assert!(out.contains("removed 1 cached archive"), "{out}");
        let (out, _) = expect(&s.run(&["runtimes", "-q"]), 0);
        assert_eq!(out, "sniper\n");
    }
}
