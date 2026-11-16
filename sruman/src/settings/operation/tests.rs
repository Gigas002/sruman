//! Resolver details; the full grammar is tested from argv in `cli/tests.rs`.

use std::path::Path;

use super::*;

fn ctx() -> Context<'static> {
    Context {
        cwd: Path::new("/work"),
        home: Path::new("/home/u"),
        default_profile: None,
    }
}

fn invocation(ops: &[OpFlag], modifiers: Modifiers, targets: &[&str]) -> Invocation {
    Invocation {
        ops: ops.to_vec(),
        modifiers,
        targets: targets.iter().map(|t| (*t).to_owned()).collect(),
        ..Invocation::default()
    }
}

#[test]
fn used_lists_every_set_modifier() {
    let modifiers = Modifiers {
        refresh: 2,
        needed: true,
        profile: Some("p".into()),
        ..Modifiers::default()
    };
    assert_eq!(
        modifiers.used(),
        [Flag::Refresh, Flag::Needed, Flag::Profile]
    );
    assert!(Modifiers::default().used().is_empty());
}

#[test]
fn counts_are_capped() {
    let modifiers = Modifiers {
        nodeps: 5,
        ..Modifiers::default()
    };
    let operation = resolve(
        invocation(&[OpFlag::Remove], modifiers, &["sniper"]),
        &ctx(),
    )
    .unwrap();
    assert!(matches!(operation, Operation::Remove { nodeps: 2, .. }));
}

#[test]
fn names_and_json_support() {
    let status = Operation::Exec(Exec::Status);
    assert_eq!(status.name(), "-Xs");
    assert!(status.supports_json());
    assert!(!status.is_run());

    let shell = Operation::Exec(Exec::Shell {
        profile: "p".into(),
        debug: false,
    });
    assert_eq!(shell.name(), "-X --shell");
    assert!(shell.is_run());
    assert_eq!(
        check_json(&shell, true),
        Err(UsageError("invalid option '--json' for -X --shell".into()))
    );
    assert_eq!(check_json(&shell, false), Ok(()));
}

#[cfg(not(feature = "download"))]
#[test]
fn query_upgradable_needs_download() {
    let modifiers = Modifiers {
        sysupgrade: 1,
        ..Modifiers::default()
    };
    assert_eq!(
        resolve(invocation(&[OpFlag::Query], modifiers, &[]), &ctx()),
        Err(UsageError("invalid option '-u' for -Q".into()))
    );
}
