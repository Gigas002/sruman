//! Snapshots of every command's `--help` (PLAN 5.5c), one per feature
//! level the gates build: the full CLI (default and `--all-features`) and
//! `--no-default-features`. A changed help text shows up as a snapshot diff
//! to review (`INSTA_UPDATE=always cargo test`, or `cargo insta review`).

use clap::CommandFactory;
use sruman::cli::Cli;

/// `sruman`, `sruman runtimes`, `sruman profiles list`, … depth first.
fn render(command: &mut clap::Command, path: &str, out: &mut String) {
    let path = if path.is_empty() {
        command.get_name().to_owned()
    } else {
        format!("{path} {}", command.get_name())
    };
    out.push_str(&format!("===== {path} --help\n"));
    out.push_str(&command.render_long_help().to_string());
    out.push('\n');
    let mut subs: Vec<clap::Command> = command
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .cloned()
        .collect();
    for sub in &mut subs {
        render(sub, &path, out);
    }
}

#[test]
fn help_texts() {
    let level = if cfg!(all(
        feature = "download",
        feature = "json",
        feature = "diag"
    )) {
        "full"
    } else if cfg!(not(any(
        feature = "download",
        feature = "json",
        feature = "diag"
    ))) {
        "minimal"
    } else {
        // Not a level the gates build.
        return;
    };
    let mut command = Cli::command().term_width(100);
    command.build();
    let mut out = String::new();
    render(&mut command, "", &mut out);
    insta::assert_snapshot!(format!("help_{level}"), out);
}
