use std::path::Path;

use super::*;

fn logging(root: &Path, level: LogLevel, stderr: Option<LogLevel>) -> Logging {
    Logging {
        level,
        stderr,
        dir: root.join("logs"),
        keep: 3,
    }
}

#[test]
fn level_filter_mapping() {
    assert_eq!(level_filter(LogLevel::Error), LevelFilter::ERROR);
    assert_eq!(level_filter(LogLevel::Warn), LevelFilter::WARN);
    assert_eq!(level_filter(LogLevel::Info), LevelFilter::INFO);
    assert_eq!(level_filter(LogLevel::Debug), LevelFilter::DEBUG);
    assert_eq!(level_filter(LogLevel::Trace), LevelFilter::TRACE);
}

#[test]
fn writes_filtered_events_to_a_daily_file() {
    let root = tempfile::tempdir().unwrap();
    let (subscriber, guard) = subscriber(&logging(root.path(), LogLevel::Info, None)).unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("kept message");
        tracing::debug!("filtered message");
    });
    drop(guard);

    let files: Vec<_> = std::fs::read_dir(root.path().join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("sruman.") && name.ends_with(".log"),
        "{name}"
    );
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("kept message"), "{text}");
    assert!(!text.contains("filtered message"), "{text}");
}

#[test]
fn verbose_raises_the_file_level_too() {
    // Settings make the file level at least the stderr one.
    let root = tempfile::tempdir().unwrap();
    let (subscriber, guard) = subscriber(&logging(
        root.path(),
        LogLevel::Debug,
        Some(LogLevel::Debug),
    ))
    .unwrap();
    tracing::subscriber::with_default(subscriber, || tracing::debug!("details"));
    drop(guard);
    let file = std::fs::read_dir(root.path().join("logs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(std::fs::read_to_string(file).unwrap().contains("details"));
}

#[test]
fn unwritable_log_dir_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("logs"), "not a directory").unwrap();
    let result = subscriber(&logging(root.path(), LogLevel::Info, None));
    assert!(matches!(result, Err(LoggerError::CreateDir(_))));
}

#[test]
fn cli_format_is_plain() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("out.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_writer(file)
            .with_ansi(false)
            .event_format(CliFormat),
    );
    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::info_span!("command", name = "install").entered();
        tracing::warn!("slow mirror");
        tracing::debug!(bytes = 2, "fetched");
    });
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "sruman: warning: slow mirror\nsruman: debug: fetched bytes=2\n"
    );
}
