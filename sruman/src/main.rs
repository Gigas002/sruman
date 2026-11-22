use std::process::ExitCode;

use clap::Parser;
use sruman::{
    app,
    cli::Cli,
    logger,
    output::Printer,
    settings::{Environment, Settings},
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!("error: cannot read the working directory: {err}");
            return ExitCode::from(1);
        }
    };
    let env = Environment {
        lookup: |key: &str| std::env::var_os(key),
        cwd,
    };
    let settings = match Settings::resolve(cli.into(), env) {
        Ok(settings) => settings,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(2);
        }
    };
    let _guard = match logger::init(&settings.logging) {
        Ok(guard) => Some(guard),
        Err(err) => {
            eprintln!("warning: no app log: {err}");
            None
        }
    };

    let mut out = Printer::stdio(&settings.output);
    let code = match app::run(&settings, &mut out) {
        Ok(code) => code,
        Err(err) if err.is_broken_pipe() => 0,
        Err(err) => {
            if !err.is_reported() {
                tracing::error!("{err}");
                let _ = out.error(err.to_string());
            }
            err.exit_code()
        }
    };
    let _ = out.flush();
    ExitCode::from(code)
}
