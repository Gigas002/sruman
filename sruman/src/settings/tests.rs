use std::{collections::HashMap, ffi::OsString, fs, path::Path};

use libsruman::version::Channel;

use super::*;

/// A fake environment rooted in `home`.
fn env(home: &Path, extra: &[(&str, &str)]) -> Environment<impl Fn(&str) -> Option<OsString>> {
    let mut vars: HashMap<String, OsString> = extra
        .iter()
        .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
        .collect();
    vars.insert("HOME".into(), home.into());
    Environment {
        lookup: move |key: &str| vars.get(key).cloned(),
        cwd: home.join("work"),
    }
}

fn write_config(home: &Path, content: &str) {
    let dir = home.join(".config/sruman");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), content).unwrap();
}

fn query() -> CliOptions {
    CliOptions::new(Command::Runtimes { quiet: false })
}

#[test]
fn defaults_without_a_config() {
    let home = tempfile::tempdir().unwrap();
    let settings = Settings::resolve(query(), env(home.path(), &[])).unwrap();

    assert_eq!(settings.dirs.home, home.path());
    assert_eq!(settings.dirs.cwd, home.path().join("work"));
    assert_eq!(
        settings.dirs.config_file,
        home.path().join(".config/sruman/config.toml")
    );
    assert_eq!(
        settings.dirs.prefix_root,
        home.path().join(".local/share/sruman/prefixes")
    );
    assert_eq!(settings.channels.get("sniper"), Channel::Stable);
    assert_eq!(settings.keep_versions, DEFAULT_KEEP_VERSIONS);
    assert!(!settings.auto_install);
    assert_eq!(settings.default_profile, None);
    assert_eq!(settings.stop_timeout, Duration::from_secs(5));
    assert_eq!(settings.output.color, ColorChoice::Auto);
    assert_eq!(settings.logging.level, LogLevel::Info);
    assert_eq!(settings.logging.stderr, None);
    assert_eq!(settings.logging.keep, DEFAULT_LOG_KEEP);
    assert!(settings.catalog.get("sniper").is_some());
}

#[test]
fn config_values_apply() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        r#"
        log_level = "warn"
        default_profile = "hk"
        channel = "beta"
        keep_versions = 4
        auto_install = true
        color = "always"
        prefix_root = "~/pfx"
        stop_timeout_secs = 1
        [channels]
        scout = "steam"
        [[runtime]]
        name = "sniper"
        base_url = "https://mirror.example"
        "#,
    );
    let settings = Settings::resolve(query(), env(home.path(), &[])).unwrap();

    assert_eq!(settings.logging.level, LogLevel::Warn);
    assert_eq!(settings.default_profile.as_deref(), Some("hk"));
    assert_eq!(settings.channels.get("sniper"), Channel::Beta);
    assert_eq!(settings.channels.get("scout"), Channel::Steam);
    assert_eq!(settings.keep_versions, 4);
    assert!(settings.auto_install);
    assert_eq!(settings.output.color, ColorChoice::Always);
    assert_eq!(settings.dirs.prefix_root, home.path().join("pfx"));
    assert_eq!(settings.stop_timeout, Duration::from_secs(1));
    assert_eq!(
        settings.catalog.get("sniper").unwrap().base_url,
        "https://mirror.example"
    );
}

#[test]
fn cli_overrides_config() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "color = \"always\"\nlog_level = \"info\"");
    let cli = CliOptions {
        color: Some(ColorChoice::Never),
        verbose: 2,
        yes: true,
        json: true,
        quiet: true,
        ..query()
    };
    let settings = Settings::resolve(cli, env(home.path(), &[])).unwrap();

    assert_eq!(settings.output.color, ColorChoice::Never);
    assert!(settings.output.yes && settings.output.json && settings.output.quiet);
    assert_eq!(settings.logging.stderr, Some(LogLevel::Trace));
    assert_eq!(settings.logging.level, LogLevel::Trace);
}

#[test]
fn verbose_never_lowers_the_file_level() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "log_level = \"trace\"");
    let cli = CliOptions {
        verbose: 1,
        ..query()
    };
    let settings = Settings::resolve(cli, env(home.path(), &[])).unwrap();
    assert_eq!(settings.logging.stderr, Some(LogLevel::Debug));
    assert_eq!(settings.logging.level, LogLevel::Trace);
}

#[test]
fn no_color_turns_auto_off() {
    let home = tempfile::tempdir().unwrap();
    let settings = Settings::resolve(query(), env(home.path(), &[("NO_COLOR", "1")])).unwrap();
    assert_eq!(settings.output.color, ColorChoice::Never);

    let cli = CliOptions {
        color: Some(ColorChoice::Always),
        ..query()
    };
    let settings = Settings::resolve(cli, env(home.path(), &[("NO_COLOR", "1")])).unwrap();
    assert_eq!(settings.output.color, ColorChoice::Always);
}

#[test]
fn environment_values() {
    let home = tempfile::tempdir().unwrap();
    let settings = Settings::resolve(
        query(),
        env(
            home.path(),
            &[
                ("PATH", "/usr/bin:relative:/bin"),
                ("SHELL", "/bin/zsh"),
                ("EDITOR", "vim"),
                ("VISUAL", " code --wait "),
            ],
        ),
    )
    .unwrap();
    assert_eq!(
        settings.search_path,
        [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
    );
    assert_eq!(settings.shell, Some(PathBuf::from("/bin/zsh")));
    assert_eq!(settings.editor.as_deref(), Some("code --wait"));
}

#[test]
fn explicit_config_must_exist() {
    let home = tempfile::tempdir().unwrap();
    let cli = CliOptions {
        config: Some("missing.toml".into()),
        ..query()
    };
    let err = Settings::resolve(cli, env(home.path(), &[])).unwrap_err();
    assert!(
        matches!(err, SettingsError::Config(ConfigError::Io { .. })),
        "{err}"
    );
}

#[test]
fn explicit_config_is_relative_to_the_cwd() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("work")).unwrap();
    fs::write(home.path().join("work/c.toml"), "keep_versions = 7").unwrap();
    let cli = CliOptions {
        config: Some("c.toml".into()),
        ..query()
    };
    let settings = Settings::resolve(cli, env(home.path(), &[])).unwrap();
    assert_eq!(settings.keep_versions, 7);
    assert_eq!(settings.dirs.config_file, home.path().join("work/c.toml"));
}

#[test]
fn invalid_values_name_the_file_and_key() {
    for (content, key) in [
        ("log_keep = 0", "log_keep"),
        ("prefix_root = \"relative\"", "prefix_root"),
        ("default_profile = \"a/b\"", "default_profile"),
        ("[channels]\n\"Bad Name\" = \"beta\"", "channels"),
    ] {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), content);
        let err = Settings::resolve(query(), env(home.path(), &[])).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("config.toml"), "{message}");
        assert!(message.contains(&format!("`{key}`")), "{message}");
    }
}

#[test]
fn invalid_runtime_override() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[[runtime]]\nname = \"newrt\"");
    let err = Settings::resolve(query(), env(home.path(), &[])).unwrap_err();
    assert!(matches!(err, SettingsError::Catalog { .. }), "{err}");
}

#[test]
fn default_profile_feeds_shells() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "default_profile = \"soldier\"");
    let cli = CliOptions::new(Command::Shell {
        profile: None,
        debug: false,
    });
    let settings = Settings::resolve(cli, env(home.path(), &[])).unwrap();
    assert_eq!(
        settings.command,
        Command::Shell {
            profile: Some("soldier".into()),
            debug: false
        }
    );
}

#[test]
fn usage_errors_surface() {
    let home = tempfile::tempdir().unwrap();
    let cli = CliOptions::new(Command::Shell {
        profile: None,
        debug: false,
    });
    let err = Settings::resolve(cli, env(home.path(), &[])).unwrap_err();
    assert!(matches!(err, SettingsError::Usage(_)), "{err}");
    assert_eq!(
        err.to_string(),
        "shell needs a profile: -p <profile>, or default_profile in config.toml"
    );
}

#[test]
fn missing_home_is_an_error() {
    let env = Environment {
        lookup: |_: &str| None,
        cwd: PathBuf::from("/"),
    };
    assert!(matches!(
        Settings::resolve(query(), env),
        Err(SettingsError::Paths(_))
    ));
}
