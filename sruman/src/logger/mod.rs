//! `tracing` setup (4.2), as in lootrs: a daily app log in the log
//! directory, and with `-v`/`-vv` the same events on stderr. Commands print
//! their own results and messages; the log records what they did.

use std::{fmt as std_fmt, io};

use tracing::{Event, Level, Subscriber};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{InitError, RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, FmtContext, FormatEvent, FormatFields, format::Writer},
    layer::{Layer, SubscriberExt},
    registry::LookupSpan,
};

use crate::{config::LogLevel, settings::Logging};

#[cfg(test)]
mod tests;

/// App logs are `sruman.<date>.log`, next to the `games/` and `exec/` run
/// logs.
pub const FILE_PREFIX: &str = "sruman";
pub const FILE_SUFFIX: &str = "log";

#[derive(Debug, thiserror::Error)]
pub enum LoggerError {
    #[error("failed to create the log directory: {0}")]
    CreateDir(#[source] io::Error),

    #[error(transparent)]
    Appender(#[from] InitError),

    #[error(transparent)]
    SetGlobal(#[from] tracing::subscriber::SetGlobalDefaultError),
}

/// Installs the global subscriber. Keep the guard until exit, so buffered
/// lines reach the file.
pub fn init(logging: &Logging) -> Result<WorkerGuard, LoggerError> {
    let (subscriber, guard) = subscriber(logging)?;
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(guard)
}

/// Builds the subscriber without installing it.
pub fn subscriber(
    logging: &Logging,
) -> Result<(impl Subscriber + Send + Sync + use<>, WorkerGuard), LoggerError> {
    std::fs::create_dir_all(&logging.dir).map_err(LoggerError::CreateDir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .max_log_files(logging.keep)
        .build(&logging.dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let file_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_filter(level_filter(logging.level));
    let stderr_layer = logging.stderr.map(|level| {
        fmt::layer()
            .with_writer(io::stderr)
            .with_ansi(false)
            .event_format(CliFormat)
            .with_filter(level_filter(level))
    });
    let subscriber = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer);
    Ok((subscriber, guard))
}

pub fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

/// Stderr lines with `-v`: `sruman: debug: message`, no spans or times.
pub struct CliFormat;

impl<S, N> FormatEvent<S, N> for CliFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std_fmt::Result {
        let level = match *event.metadata().level() {
            Level::ERROR => "error",
            Level::WARN => "warning",
            Level::INFO => "info",
            Level::DEBUG => "debug",
            Level::TRACE => "trace",
        };
        write!(writer, "sruman: {level}: ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}
