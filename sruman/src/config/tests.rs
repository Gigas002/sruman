use super::*;

#[test]
fn empty_file_is_all_defaults() {
    assert_eq!(parse("").unwrap(), FileConfig::default());
}

#[test]
fn parses_every_key() {
    let config = parse(
        r#"
        log_level = "debug"
        log_keep = 5
        default_profile = "sniper"
        channel = "beta"
        keep_versions = 3
        auto_install = true
        color = "never"
        prefix_root = "~/prefixes"
        stop_timeout_secs = 9

        [channels]
        scout = "steam"

        [[runtime]]
        name = "sniper"
        base_url = "https://mirror.example"
        "#,
    )
    .unwrap();

    assert_eq!(config.log_level, Some(LogLevel::Debug));
    assert_eq!(config.log_keep, Some(5));
    assert_eq!(config.default_profile.as_deref(), Some("sniper"));
    assert_eq!(config.channel, Some(Channel::Beta));
    assert_eq!(config.channels.get("scout"), Some(&Channel::Steam));
    assert_eq!(config.keep_versions, Some(3));
    assert_eq!(config.auto_install, Some(true));
    assert_eq!(config.color, Some(ColorChoice::Never));
    assert_eq!(config.prefix_root, Some(PathBuf::from("~/prefixes")));
    assert_eq!(config.stop_timeout_secs, Some(9));
    assert_eq!(config.runtime.len(), 1);
    assert_eq!(
        config.runtime[0].base_url.as_deref(),
        Some("https://mirror.example")
    );
}

#[test]
fn unknown_keys_are_rejected() {
    let err = parse("keep_version = 3").unwrap_err();
    assert!(err.to_string().contains("keep_version"), "{err}");
}

#[test]
fn bad_channel_is_rejected() {
    assert!(parse("channel = \"nightly\"").is_err());
}

#[test]
fn missing_file_is_none_unless_required() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    assert_eq!(load(&path, false).unwrap(), None);
    assert!(matches!(load(&path, true), Err(ConfigError::Io { .. })));
}

#[test]
fn load_reports_the_path_on_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE_NAME);
    std::fs::write(&path, "keep_versions = \"two\"").unwrap();
    let err = load(&path, false).unwrap_err();
    assert!(err.to_string().contains(&*path.to_string_lossy()), "{err}");
}
