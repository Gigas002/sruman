use std::{fs, os::unix::fs::PermissionsExt};

use serde_json::json;

use super::*;
use crate::{store::Installed, version::BuildId};

/// Writes an executable `sh` script. Callers hold `test_lock::spawns()`:
/// a fork elsewhere while the file is open for writing would make its exec
/// fail with ETXTBSY.
fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn installed(name: &str, dir: &Path) -> Installed {
    let build: BuildId = "3.0.20261021.1".parse().unwrap();
    Installed {
        name: name.to_owned(),
        build,
        dir: dir.to_owned(),
        meta: None,
        is_current: true,
    }
}

fn kernel_root(values: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (rel, value) in values {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{value}\n")).unwrap();
    }
    tmp
}

#[test]
fn kernel_sysctls_are_read_and_explained() {
    let fine = kernel_root(&[
        ("kernel/unprivileged_userns_clone", "1"),
        ("user/max_user_namespaces", "60282"),
    ]);
    let kernel = Kernel::read_in(fine.path());
    assert_eq!(
        kernel,
        Kernel {
            unprivileged_userns_clone: Some(1),
            max_user_namespaces: Some(60282),
            apparmor_restrict_unprivileged_userns: None,
        }
    );
    let findings = kernel.findings(None);
    assert_eq!(worst(&findings), Level::Ok);
    assert_eq!(findings.len(), 1);

    // A kernel without any of the knobs (upstream, no Debian patch).
    assert_eq!(
        worst(&Kernel::read_in(Path::new("/nonexistent")).findings(None)),
        Level::Ok
    );

    let disabled = kernel_root(&[
        ("kernel/unprivileged_userns_clone", "0"),
        ("user/max_user_namespaces", "0"),
    ]);
    let findings = Kernel::read_in(disabled.path()).findings(None);
    assert_eq!(findings.len(), 2);
    assert!(
        findings
            .iter()
            .all(|f| f.level == Level::Error && f.hint.is_some())
    );
    assert!(findings[0].message.contains("max_user_namespaces"));
    assert!(findings[1].message.contains("unprivileged_userns_clone"));

    let ubuntu = kernel_root(&[
        ("user/max_user_namespaces", "60282"),
        ("kernel/apparmor_restrict_unprivileged_userns", "1"),
    ]);
    let bwrap =
        Path::new("/store/sniper/3.0/pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap");
    let findings = Kernel::read_in(ubuntu.path()).findings(Some(bwrap));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].level, Level::Warn);
    let hint = findings[0].hint.as_deref().unwrap();
    assert!(hint.contains(&bwrap.display().to_string()), "{hint}");
    assert!(hint.contains("userns,"), "{hint}");
}

#[test]
fn tools_are_found_for_each_runtime_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let sniper = tmp.path().join("sniper");
    let soldier = tmp.path().join("soldier");
    let scout = tmp.path().join("scout");
    let ldlp = tmp.path().join("scout-ldlp");
    for (dir, rel) in [
        (&sniper, "pressure-vessel/bin"),
        (&sniper, "pressure-vessel-arm64/bin"),
        (&soldier, "pressure-vessel/bin"),
        (&ldlp, "amd64/usr/bin"),
    ] {
        let bin = dir.join(rel);
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join(SYSTEM_INFO), "").unwrap();
    }
    fs::create_dir_all(&scout).unwrap();

    let container = Runtimes {
        runtime: installed("sniper", &sniper),
        host: None,
        entry: Entry::Container,
    };
    assert_eq!(
        tools_for(&container, Arch::X86_64),
        Some(Tools::PressureVessel(sniper.join("pressure-vessel")))
    );
    assert_eq!(
        tools_for(&container, Arch::Arm64),
        Some(Tools::PressureVessel(sniper.join("pressure-vessel-arm64")))
    );

    let on_soldier = Runtimes {
        runtime: installed("scout", &scout),
        host: Some(installed("soldier", &soldier)),
        entry: Entry::ScoutOnSoldier,
    };
    let tools = tools_for(&on_soldier, Arch::X86_64).unwrap();
    assert_eq!(
        tools,
        Tools::PressureVessel(soldier.join("pressure-vessel"))
    );
    // soldier has no arm64 build of pressure-vessel here.
    assert_eq!(tools_for(&on_soldier, Arch::Arm64), None);

    let plain = Runtimes {
        runtime: installed("scout-ldlp", &ldlp),
        host: None,
        entry: Entry::Ldlp,
    };
    let tools = tools_for(&plain, Arch::X86_64).unwrap();
    assert_eq!(
        tools.argv(SYSTEM_INFO, &["--verbose"]),
        vec![
            ldlp.join("run.sh"),
            ldlp.join("amd64/usr/bin").join(SYSTEM_INFO),
            PathBuf::from("--verbose"),
        ]
    );
    assert_eq!(tools.bwrap(), None);
    assert_eq!(tools_for(&plain, Arch::Arm64), None);

    // A container build without the tools.
    let bare = Runtimes {
        runtime: installed("scout", &scout),
        host: None,
        entry: Entry::Container,
    };
    assert_eq!(tools_for(&bare, Arch::X86_64), None);
}

#[test]
fn run_tool_captures_output_and_enforces_the_timeout() {
    let _spawn = crate::test_lock::spawns();
    let tmp = tempfile::tempdir().unwrap();
    let ok = tmp.path().join("ok");
    let fail = tmp.path().join("fail");
    let slow = tmp.path().join("slow");
    let leaky = tmp.path().join("leaky");
    script(&ok, "echo out; echo err >&2");
    script(
        &fail,
        "echo first >&2; echo 'the reason' >&2; echo >&2; exit 3",
    );
    script(&slow, "exec sleep 30");
    // Leaves a grandchild holding stdout open after exiting.
    script(&leaky, "sleep 5 & echo done");

    let out = run_tool(&[ok], Duration::from_secs(10)).unwrap();
    assert!(out.success());
    assert_eq!(out.stdout, b"out\n");
    assert_eq!(out.stderr, "err\n");

    let out = run_tool(&[fail], Duration::from_secs(10)).unwrap();
    assert!(!out.success());
    let message = out.describe_failure(Duration::from_secs(10));
    assert!(message.ends_with(": the reason"), "{message}");
    assert!(message.contains('3'), "{message}");

    let started = Instant::now();
    let out = run_tool(&[slow], Duration::from_millis(300)).unwrap();
    assert!(out.status.is_none());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        out.describe_failure(Duration::from_secs(60))
            .starts_with("timed out after 60s")
    );

    let started = Instant::now();
    let out = run_tool(&[leaky], Duration::from_secs(10)).unwrap();
    assert!(out.success());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );

    assert_eq!(
        run_tool(&[tmp.path().join("missing")], Duration::from_secs(1))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(run_tool(&[], Duration::from_secs(1)).is_err());
}

/// A fake pressure-vessel directory with tool scripts.
fn fake_tools(root: &Path, bwrap: &str, requirements: &str, system_info: &str) -> Tools {
    let pv = root.join("pressure-vessel");
    script(&pv.join("libexec/steam-runtime-tools-0/srt-bwrap"), bwrap);
    script(&pv.join("bin").join(CHECK_REQUIREMENTS), requirements);
    script(&pv.join("bin").join(SYSTEM_INFO), system_info);
    Tools::PressureVessel(pv)
}

#[test]
fn tool_checks_report_success_and_failure() {
    let _spawn = crate::test_lock::spawns();
    let tmp = tempfile::tempdir().unwrap();
    let t = Duration::from_secs(10);

    let good = fake_tools(
        &tmp.path().join("good"),
        r#"[ "$1" = --unshare-user ] && [ "$4" = / ] && [ "$5" = true ] && exit 0; exit 9"#,
        "exit 0",
        r#"echo '{"container": {"type": "none", "bubblewrap_ok": true}, "locales-ok": true}'"#,
    );
    assert_eq!(bwrap_probe(&good, t).level, Level::Ok);
    assert_eq!(check_requirements(&good, t).level, Level::Ok);
    let info = system_info(&good, t).unwrap();
    assert!(info.raw.contains("bubblewrap_ok"));
    let findings = info.findings();
    assert_eq!(worst(&findings), Level::Ok);
    assert_eq!(findings.len(), 1);

    let bad = fake_tools(
        &tmp.path().join("bad"),
        "echo 'bwrap: setting up uid map: Permission denied' >&2; exit 1",
        "echo 'This computer does not meet the requirements: missing CMPXCHG16B' >&2; exit 71",
        "echo not json",
    );
    let probe = bwrap_probe(&bad, t);
    assert_eq!(probe.level, Level::Error);
    assert!(probe.message.contains("Permission denied"), "{probe:?}");
    assert!(probe.hint.is_some());
    let requirements = check_requirements(&bad, t);
    assert_eq!(requirements.level, Level::Error);
    assert!(
        requirements.message.contains("CMPXCHG16B"),
        "{requirements:?}"
    );
    assert!(matches!(system_info(&bad, t), Err(DiagError::BadOutput(_))));

    let crashing = fake_tools(
        &tmp.path().join("crash"),
        "exit 0",
        "exit 0",
        "echo boom >&2; exit 2",
    );
    let err = system_info(&crashing, t).unwrap_err();
    assert!(
        matches!(&err, DiagError::Failed(m) if m.ends_with(": boom")),
        "{err}"
    );

    // scout-ldlp: no container, so no probe.
    let ldlp = Tools::Ldlp(tmp.path().join("ldlp"));
    assert_eq!(bwrap_probe(&ldlp, t).level, Level::Ok);
    assert_eq!(check_requirements(&ldlp, t).level, Level::Warn);
}

#[test]
fn system_info_is_summarized() {
    // The shape of a real report, trimmed to what the summary reads.
    let healthy = json!({
        "container": {"type": "none", "bubblewrap_ok": true},
        "architectures": {
            "x86_64-linux-gnu": {"can-run": true},
            "i386-linux-gnu": {"can-run": true}
        },
        "cpu-features": {"x86-64": true, "sse3": true, "cmpxchg16b": true},
        "locales-ok": true
    });
    let findings = summarize(&healthy);
    assert_eq!(worst(&findings), Level::Ok);

    // A host without 32-bit libraries (common, and fine for 64-bit games).
    let no_multilib = json!({
        "container": {"type": "none", "bubblewrap_ok": true},
        "architectures": {
            "x86_64-linux-gnu": {"can-run": true},
            "i386-linux-gnu": {"can-run": false}
        }
    });
    let findings = summarize(&no_multilib);
    assert_eq!(worst(&findings), Level::Warn);
    assert!(findings.iter().any(|f| f.message.contains("32-bit")));

    let broken = json!({
        "container": {"type": "flatpak", "bubblewrap_ok": false},
        "architectures": {"x86_64-linux-gnu": {"can-run": false}},
        "cpu-features": {"x86-64": true, "sse3": false, "cmpxchg16b": false},
        "locales-ok": false
    });
    let findings = summarize(&broken);
    let levels: Vec<Level> = findings.iter().map(|f| f.level).collect();
    assert_eq!(
        levels,
        [
            Level::Error,
            Level::Warn,
            Level::Error,
            Level::Error,
            Level::Warn
        ],
        "{findings:#?}"
    );
    assert!(findings[1].message.contains("flatpak"));
    assert!(
        findings[3].message.ends_with("cmpxchg16b, sse3"),
        "{}",
        findings[3].message
    );

    // Unknown or missing fields are skipped, not errors.
    assert!(summarize(&json!({})).is_empty());
    assert!(summarize(&json!([1, 2])).is_empty());
}

/// Runs the real tools of an installed container build:
/// `SRUMAN_REAL_BUILD=<store>/sniper/<build> cargo test --all-features -- --ignored real_doctor`.
#[test]
#[ignore = "needs a real container runtime build (SRUMAN_REAL_BUILD)"]
fn real_doctor() {
    let Some(dir) = std::env::var_os("SRUMAN_REAL_BUILD") else {
        panic!("set SRUMAN_REAL_BUILD to an installed container runtime build");
    };
    let _spawn = crate::test_lock::spawns();
    let arch = Arch::current().unwrap();
    let tools = pressure_vessel(Path::new(&dir), arch).expect("no pressure-vessel in the build");
    let mut findings = Kernel::read().findings(tools.bwrap().as_deref());
    findings.push(bwrap_probe(&tools, TOOL_TIMEOUT));
    findings.push(check_requirements(&tools, TOOL_TIMEOUT));
    let info = system_info(&tools, TOOL_TIMEOUT).unwrap();
    findings.extend(info.findings());
    for f in &findings {
        eprintln!("[{}] {}: {}", f.level, f.check, f.message);
        if let Some(hint) = &f.hint {
            eprintln!("    hint: {hint}");
        }
    }
    assert!(worst(&findings) < Level::Error, "{findings:#?}");
}
