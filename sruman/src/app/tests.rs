use std::{io, path::PathBuf};

use libsruman::{
    model::{Issue, Subject},
    prefix::PrefixError,
    store::StoreError,
};

use super::*;

#[test]
fn exit_codes() {
    for (err, code) in [
        (AppError::Invalid("x".into()), 2),
        (AppError::Failed("x".into()), 1),
        (AppError::NotRunning("x".into()), 3),
        (AppError::Busy("x".into()), 4),
        (AppError::Cancelled, 1),
        (AppError::Reported(7), 7),
        (
            AppError::Supervisor {
                code: 4,
                message: "x".into(),
            },
            4,
        ),
        (AppError::Output(io::Error::other("x")), 1),
    ] {
        assert_eq!(err.exit_code(), code, "{err:?}");
    }
    assert!(AppError::Output(io::ErrorKind::BrokenPipe.into()).is_broken_pipe());
    assert!(!AppError::Output(io::Error::other("x")).is_broken_pipe());
    assert!(AppError::Reported(1).is_reported() && !AppError::Cancelled.is_reported());
}

#[test]
fn library_errors_map_to_codes() {
    let store = |err: StoreError| AppError::from(err).exit_code();
    assert_eq!(store(StoreError::Busy(PathBuf::from("/l"))), 4);
    assert_eq!(
        store(StoreError::InUse {
            name: "sniper".into(),
            build: "3.0.1".parse().unwrap()
        }),
        4
    );
    assert_eq!(
        store(StoreError::NotInstalled {
            name: "sniper".into()
        }),
        2
    );
    let prefix = |err: PrefixError| AppError::from(err).exit_code();
    assert_eq!(prefix(PrefixError::Busy(PathBuf::from("/p"))), 4);
    assert_eq!(prefix(PrefixError::Relative(PathBuf::from("p"))), 2);
    let launch = AppError::from(LaunchError {
        problems: vec!["a".into(), "b".into()],
    });
    assert_eq!(launch.exit_code(), 2);
    assert_eq!(launch.to_string(), "not launching:\n  a\n  b");
}

#[test]
fn issue_text_drops_the_severity() {
    let error = Issue::error(Subject::Game("g".into()), "bad");
    let warning = Issue::warning(Subject::Profile("p".into()), "meh");
    assert_eq!(issue_text(&error), "game `g`: bad");
    assert_eq!(issue_text(&warning), "profile `p`: meh");
}
