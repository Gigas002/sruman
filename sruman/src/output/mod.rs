//! The printer (4.3): pacman's transaction look (`::` headings, tables,
//! sizes, `[Y/n]` prompts), `warning:`/`error:` lines, a progress bar for
//! downloads and extraction, colors on terminals, and `--json`.
//!
//! Results go to stdout, messages and progress to stderr, so
//! `sruman runtimes | …` and `--json` stay clean.

use std::{
    io::{self, BufRead, IsTerminal, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use libsruman::progress::{Phase, Progress};

use crate::{config::ColorChoice, settings::Output};

#[cfg(test)]
mod tests;

/// Where the printer writes and reads.
pub struct Printer {
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    input: Box<dyn BufRead>,
    /// Colors on stdout / stderr.
    color_out: bool,
    color_err: bool,
    /// stderr is a terminal: the progress bar redraws in place.
    err_tty: bool,
    /// stdin is a terminal: prompts can be answered.
    interactive: bool,
    pub json: bool,
    pub quiet: bool,
    /// `--yes`: every prompt takes its default.
    pub yes: bool,
}

impl Printer {
    /// The real terminal.
    pub fn stdio(output: &Output) -> Self {
        let colors = |tty: bool| match output.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => tty,
        };
        let out_tty = io::stdout().is_terminal();
        let err_tty = io::stderr().is_terminal();
        Self {
            out: Box::new(io::stdout()),
            err: Box::new(io::stderr()),
            input: Box::new(io::BufReader::new(io::stdin())),
            color_out: colors(out_tty),
            color_err: colors(err_tty),
            err_tty,
            interactive: io::stdin().is_terminal(),
            json: output.json,
            quiet: output.quiet,
            yes: output.yes,
        }
    }

    /// Writes into buffers and reads answers from `input`, for tests.
    pub fn capture(output: &Output, input: &str, interactive: bool) -> (Self, Buffer, Buffer) {
        let (out, err) = (Buffer::default(), Buffer::default());
        let color = output.color == ColorChoice::Always;
        let printer = Self {
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            input: Box::new(io::Cursor::new(input.to_owned().into_bytes())),
            color_out: color,
            color_err: color,
            err_tty: false,
            interactive,
            json: output.json,
            quiet: output.quiet,
            yes: output.yes,
        };
        (printer, out, err)
    }

    /// Whether a question can be asked (and wasn't answered by `--yes`).
    pub fn can_ask(&self) -> bool {
        self.interactive && !self.yes
    }

    /// A result line on stdout.
    pub fn line(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        writeln!(self.out, "{}", text.as_ref())
    }

    /// Lines on stdout, unless `-q`.
    pub fn info(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.line(text)
    }

    /// `:: text`, pacman's step heading, on stdout (unless `-q`).
    pub fn heading(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        let marker = paint(self.color_out, BLUE_BOLD, "::");
        let text = paint(self.color_out, BOLD, text.as_ref());
        writeln!(self.out, "{marker} {text}")
    }

    pub fn warning(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        let label = paint(self.color_err, YELLOW_BOLD, "warning:");
        writeln!(self.err, "{label} {}", text.as_ref())
    }

    pub fn error(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        let label = paint(self.color_err, RED_BOLD, "error:");
        writeln!(self.err, "{label} {}", text.as_ref())
    }

    /// A message on stderr, such as "started …" notes.
    pub fn note(&mut self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        writeln!(self.err, "{}", text.as_ref())
    }

    /// Bold on stdout.
    pub fn bold(&self, text: &str) -> String {
        paint(self.color_out, BOLD, text)
    }

    /// Green on stdout, for "installed", "ok".
    pub fn green(&self, text: &str) -> String {
        paint(self.color_out, GREEN_BOLD, text)
    }

    /// Yellow on stdout, for warnings in tables.
    pub fn yellow(&self, text: &str) -> String {
        paint(self.color_out, YELLOW_BOLD, text)
    }

    /// Red on stdout, for errors in tables.
    pub fn red(&self, text: &str) -> String {
        paint(self.color_out, RED_BOLD, text)
    }

    /// `:: question [Y/n] `: true for yes. `--yes` takes the default; so
    /// does an empty answer. Without a terminal, the default too (pacman
    /// does the same).
    pub fn confirm(&mut self, question: &str, default: bool) -> io::Result<bool> {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        let marker = paint(self.color_out, BLUE_BOLD, "::");
        write!(
            self.out,
            "{marker} {} {hint} ",
            paint(self.color_out, BOLD, question)
        )?;
        if self.yes || !self.interactive {
            writeln!(self.out)?;
            return Ok(default);
        }
        self.out.flush()?;
        let mut answer = String::new();
        self.input.read_line(&mut answer)?;
        Ok(match answer.trim().to_ascii_lowercase().as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }

    /// Aligned columns on stdout, two spaces apart; the last column isn't
    /// padded. Widths count characters, not escape codes.
    pub fn table(&mut self, rows: &[Vec<String>]) -> io::Result<()> {
        for line in table(rows) {
            self.line(line)?;
        }
        Ok(())
    }

    /// A progress bar on stderr for one transaction.
    pub fn progress(&mut self) -> Bar<'_> {
        Bar {
            printer: self,
            item: String::new(),
            phase: None,
            total: None,
            done: 0,
            drawn: None,
            width: 0,
        }
    }

    /// `value` as pretty JSON on stdout.
    #[cfg(feature = "json")]
    pub fn json(&mut self, value: &impl serde::Serialize) -> io::Result<()> {
        let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        writeln!(self.out, "{text}")
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()?;
        self.err.flush()
    }
}

const BOLD: &str = "1";
const BLUE_BOLD: &str = "1;34";
const GREEN_BOLD: &str = "1;32";
const YELLOW_BOLD: &str = "1;33";
const RED_BOLD: &str = "1;31";

fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Characters on screen, without escape codes.
fn visible_len(text: &str) -> usize {
    let mut len = 0;
    let mut escape = false;
    for c in text.chars() {
        match (escape, c) {
            (false, '\x1b') => escape = true,
            (true, 'm') => escape = false,
            (true, _) => {}
            (false, _) => len += 1,
        }
    }
    len
}

/// Rows as aligned lines; see [`Printer::table`].
pub fn table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| visible_len(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (c, cell) in row.iter().enumerate() {
                if c + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(cell);
                    line.push_str(&" ".repeat(widths[c] - visible_len(cell) + 2));
                }
            }
            line.trim_end().to_owned()
        })
        .collect()
}

/// Bytes as pacman prints them: `512 B`, `12.40 KiB`, `250.31 MiB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

/// Download and extraction progress, one item at a time. On a terminal it
/// redraws one line in place (at most every 100 ms); otherwise it prints a
/// line per phase.
pub struct Bar<'a> {
    printer: &'a mut Printer,
    item: String,
    phase: Option<Phase>,
    total: Option<u64>,
    done: u64,
    drawn: Option<Instant>,
    /// Length of the last drawn line, to blank it.
    width: usize,
}

const BAR_CELLS: usize = 24;
const REDRAW: Duration = Duration::from_millis(100);

impl Bar<'_> {
    fn draw(&mut self, force: bool) {
        if self.printer.quiet || !self.printer.err_tty {
            return;
        }
        if !force && self.drawn.is_some_and(|at| at.elapsed() < REDRAW) {
            return;
        }
        self.drawn = Some(Instant::now());
        let phase = self.phase.map_or("", Phase::as_str);
        let (bar, numbers) = match self.total {
            Some(total) if total > 0 => {
                let ratio = (self.done as f64 / total as f64).min(1.0);
                let filled = (ratio * BAR_CELLS as f64) as usize;
                (
                    format!("[{}{}]", "#".repeat(filled), "-".repeat(BAR_CELLS - filled)),
                    format!(
                        "{:>3}%  {} / {}",
                        (ratio * 100.0) as u32,
                        size(self.done),
                        size(total)
                    ),
                )
            }
            _ => (String::new(), size(self.done)),
        };
        let line = format!(" {:<36} {phase:<11} {bar} {numbers}", self.item);
        let pad = self.width.saturating_sub(line.len());
        self.width = line.len();
        let _ = write!(self.printer.err, "\r{line}{}", " ".repeat(pad));
        let _ = self.printer.err.flush();
    }
}

impl Progress for Bar<'_> {
    fn begin(&mut self, item: &str) {
        item.clone_into(&mut self.item);
        self.phase = None;
        self.width = 0;
    }

    fn phase(&mut self, phase: Phase, total: Option<u64>) {
        self.phase = Some(phase);
        self.total = total;
        self.done = 0;
        if self.printer.err_tty {
            self.draw(true);
        } else if !self.printer.quiet {
            let _ = writeln!(self.printer.err, "{} {}", phase.as_str(), self.item);
        }
    }

    fn advance(&mut self, n: u64) {
        self.done += n;
        self.draw(false);
    }

    fn end(&mut self, ok: bool) {
        if self.printer.quiet || !self.printer.err_tty {
            return;
        }
        self.draw(true);
        let status = if ok { "done" } else { "failed" };
        let _ = writeln!(self.printer.err, "  {status}");
    }
}

/// A shared, growable byte buffer that implements [`Write`]; clones see the
/// same bytes. For tests.
#[derive(Debug, Clone, Default)]
pub struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    pub fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
