use std::process::ExitCode;

use sruman::{
    cli::Cli,
    settings::{Environment, Settings},
};

fn main() -> ExitCode {
    let cli = Cli::try_parse_args(std::env::args_os()).unwrap_or_else(|err| err.exit());
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

    // Logging (4.2) and the commands (4.4) come next.
    eprintln!("{} not implemented yet", settings.operation.name());
    ExitCode::from(1)
}
