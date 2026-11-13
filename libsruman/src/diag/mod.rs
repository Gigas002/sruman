//! Doctor checks (`sruman -D`): can this machine run the runtimes?
//!
//! - The kernel's user-namespace switches: `kernel.unprivileged_userns_clone`
//!   (Debian/Arch), `user.max_user_namespaces`, and Ubuntu's AppArmor
//!   restriction `kernel.apparmor_restrict_unprivileged_userns`.
//! - A live probe: the runtime's own `srt-bwrap` creating a user namespace.
//!   This is the verdict; the sysctls only explain it.
//! - The runtime's `steam-runtime-check-requirements` and
//!   `steam-runtime-system-info`, the latter summarized into findings.
//!
//! Every check returns [`Finding`]s; the caller prints them.

use std::{
    fmt,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    catalog::{Arch, Entry},
    launch::Runtimes,
};

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ok => "ok",
            Self::Warn => "warning",
            Self::Error => "error",
        })
    }
}

/// One result of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Short name of the check, like `userns` or `system-info`.
    pub check: &'static str,
    pub level: Level,
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
}

impl Finding {
    fn new(check: &'static str, level: Level, message: impl Into<String>) -> Self {
        Self {
            check,
            level,
            message: message.into(),
            hint: None,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// The worst level among `findings` (`Ok` when empty).
pub fn worst(findings: &[Finding]) -> Level {
    findings.iter().map(|f| f.level).max().unwrap_or(Level::Ok)
}

// ---------------------------------------------------------------- kernel

/// The user-namespace sysctls; `None` where the knob doesn't exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kernel {
    /// `kernel/unprivileged_userns_clone` (Debian and Arch kernels).
    pub unprivileged_userns_clone: Option<u64>,
    /// `user/max_user_namespaces`.
    pub max_user_namespaces: Option<u64>,
    /// `kernel/apparmor_restrict_unprivileged_userns` (Ubuntu 23.10+).
    pub apparmor_restrict_unprivileged_userns: Option<u64>,
}

impl Kernel {
    /// Reads the sysctls from `/proc/sys`.
    pub fn read() -> Self {
        Self::read_in(Path::new("/proc/sys"))
    }

    /// Reads the sysctls under `sys_root` (`/proc/sys`).
    pub fn read_in(sys_root: &Path) -> Self {
        let read = |rel: &str| {
            std::fs::read_to_string(sys_root.join(rel))
                .ok()
                .and_then(|s| s.trim().parse().ok())
        };
        Self {
            unprivileged_userns_clone: read("kernel/unprivileged_userns_clone"),
            max_user_namespaces: read("user/max_user_namespaces"),
            apparmor_restrict_unprivileged_userns: read(
                "kernel/apparmor_restrict_unprivileged_userns",
            ),
        }
    }

    /// What the sysctls say. `bwrap` is the runtime's `srt-bwrap`, named in
    /// the AppArmor hint when known.
    pub fn findings(&self, bwrap: Option<&Path>) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.max_user_namespaces == Some(0) {
            out.push(
                Finding::new(
                    "kernel",
                    Level::Error,
                    "user namespaces are disabled (user.max_user_namespaces = 0)",
                )
                .hint("enable them: `sysctl -w user.max_user_namespaces=10000` (as root), and set it in /etc/sysctl.d to keep it"),
            );
        }
        if self.unprivileged_userns_clone == Some(0) {
            out.push(
                Finding::new(
                    "kernel",
                    Level::Error,
                    "unprivileged user namespaces are disabled (kernel.unprivileged_userns_clone = 0)",
                )
                .hint("enable them: `sysctl -w kernel.unprivileged_userns_clone=1` (as root), and set it in /etc/sysctl.d to keep it; or install a setuid bwrap"),
            );
        }
        if self.apparmor_restrict_unprivileged_userns == Some(1) {
            let target = bwrap.map_or_else(
                || {
                    "the runtime's pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap"
                        .to_owned()
                },
                |p| p.display().to_string(),
            );
            out.push(
                Finding::new(
                    "kernel",
                    Level::Warn,
                    "AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns = 1)",
                )
                .hint(format!(
                    "if the bwrap probe fails, give {target} an AppArmor profile with `userns,` (Ubuntu ships one for Steam's own copy), or install a setuid bwrap"
                )),
            );
        }
        if out.is_empty() {
            out.push(Finding::new(
                "kernel",
                Level::Ok,
                "unprivileged user namespaces are allowed",
            ));
        }
        out
    }
}

// ----------------------------------------------------------------- tools

/// Where a runtime keeps its diagnostic tools, and how to run them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tools {
    /// A container runtime's `pressure-vessel` (or `pressure-vessel-arm64`)
    /// directory; tools run directly from `bin/`.
    PressureVessel(PathBuf),
    /// scout-ldlp: tools live in `<build>/amd64/usr/bin` and need the
    /// runtime's libraries, so they run through `<build>/run.sh`.
    Ldlp(PathBuf),
}

/// The tools for a resolved profile runtime (`launch::resolve_runtimes`).
/// Scout uses its soldier's. `None` when the build has none for `arch`.
pub fn tools_for(runtimes: &Runtimes, arch: Arch) -> Option<Tools> {
    let tools = match runtimes.entry {
        Entry::Container => pressure_vessel(&runtimes.runtime.dir, arch),
        Entry::ScoutOnSoldier => pressure_vessel(&runtimes.host.as_ref()?.dir, arch),
        Entry::Ldlp => (arch == Arch::X86_64).then(|| Tools::Ldlp(runtimes.runtime.dir.clone())),
    }?;
    tools.program(SYSTEM_INFO).is_file().then_some(tools)
}

/// The tools of a container build directory.
pub fn pressure_vessel(build_dir: &Path, arch: Arch) -> Option<Tools> {
    let dir = build_dir.join(match arch {
        Arch::X86_64 => "pressure-vessel",
        Arch::Arm64 => "pressure-vessel-arm64",
    });
    dir.is_dir().then_some(Tools::PressureVessel(dir))
}

const SYSTEM_INFO: &str = "steam-runtime-system-info";
const CHECK_REQUIREMENTS: &str = "steam-runtime-check-requirements";

impl Tools {
    /// The tool's file.
    pub fn program(&self, tool: &str) -> PathBuf {
        match self {
            Self::PressureVessel(dir) => dir.join("bin").join(tool),
            Self::Ldlp(build) => build.join("amd64/usr/bin").join(tool),
        }
    }

    /// The runtime's bubblewrap, for container runtimes.
    pub fn bwrap(&self) -> Option<PathBuf> {
        match self {
            Self::PressureVessel(dir) => {
                let path = dir.join("libexec/steam-runtime-tools-0/srt-bwrap");
                path.is_file().then_some(path)
            }
            Self::Ldlp(_) => None,
        }
    }

    /// The argv that runs `tool` with `args`.
    pub fn argv(&self, tool: &str, args: &[&str]) -> Vec<PathBuf> {
        let mut argv = match self {
            Self::PressureVessel(_) => vec![self.program(tool)],
            Self::Ldlp(build) => vec![build.join(Entry::Ldlp.marker()), self.program(tool)],
        };
        argv.extend(args.iter().map(PathBuf::from));
        argv
    }
}

/// What a tool did.
#[derive(Debug)]
pub struct ToolOutput {
    /// `None` when it was killed for running too long.
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl ToolOutput {
    pub fn success(&self) -> bool {
        self.status.is_some_and(|s| s.success())
    }

    /// The last non-empty line of stderr, for messages.
    fn stderr_tail(&self) -> Option<&str> {
        self.stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
    }

    fn describe_failure(&self, timeout: Duration) -> String {
        let why = match self.status {
            None => format!("timed out after {}s", timeout.as_secs()),
            Some(status) => status.to_string(),
        };
        match self.stderr_tail() {
            Some(tail) => format!("{why}: {tail}"),
            None => why,
        }
    }
}

/// Runs `argv` with no stdin, capturing its output, killing it after
/// `timeout`.
pub fn run_tool(argv: &[PathBuf], timeout: Duration) -> io::Result<ToolOutput> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().map(read_all);
    let stderr = child.stderr.take().map(read_all);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            // Already exited if this fails; the wait reaps it either way.
            let _ = child.kill();
            child.wait()?;
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    // A grandchild may hold the pipes open after the tool exits; give the
    // readers a moment, then leave them behind rather than hang.
    let readers_deadline = Instant::now() + Duration::from_secs(1);
    let join = |h: Option<thread::JoinHandle<Vec<u8>>>| {
        let h = h?;
        while !h.is_finished() && Instant::now() < readers_deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() { h.join().ok() } else { None }
    };
    let (stdout, stderr) = (
        join(stdout).unwrap_or_default(),
        join(stderr).unwrap_or_default(),
    );
    Ok(ToolOutput {
        status,
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Reads a pipe to the end on a thread.
fn read_all(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

// ---------------------------------------------------------------- checks

/// How long each tool may run. system-info probes graphics drivers and
/// takes a few seconds.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// The live probe: the runtime's `srt-bwrap` creating a user namespace.
pub fn bwrap_probe(tools: &Tools, timeout: Duration) -> Finding {
    let Some(bwrap) = tools.bwrap() else {
        return Finding::new("bwrap", Level::Ok, "this runtime runs without a container");
    };
    let argv: Vec<PathBuf> = [
        bwrap.as_os_str(),
        "--unshare-user".as_ref(),
        "--ro-bind".as_ref(),
        "/".as_ref(),
        "/".as_ref(),
        "true".as_ref(),
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    match run_tool(&argv, timeout) {
        Ok(out) if out.success() => Finding::new(
            "bwrap",
            Level::Ok,
            "the runtime's bubblewrap can create a user namespace",
        ),
        Ok(out) => Finding::new(
            "bwrap",
            Level::Error,
            format!(
                "the runtime's bubblewrap can't create a container ({})",
                out.describe_failure(timeout)
            ),
        )
        .hint("see the kernel findings above; containers need unprivileged user namespaces or a setuid bwrap"),
        Err(e) => Finding::new(
            "bwrap",
            Level::Error,
            format!("can't run {}: {e}", bwrap.display()),
        ),
    }
}

/// `steam-runtime-check-requirements`: CPU features and the like.
pub fn check_requirements(tools: &Tools, timeout: Duration) -> Finding {
    let argv = tools.argv(CHECK_REQUIREMENTS, &[]);
    if !tools.program(CHECK_REQUIREMENTS).is_file() {
        return Finding::new(
            "requirements",
            Level::Warn,
            format!("{CHECK_REQUIREMENTS} is missing from this runtime"),
        );
    }
    match run_tool(&argv, timeout) {
        Ok(out) if out.success() => Finding::new(
            "requirements",
            Level::Ok,
            "the machine meets the runtime's requirements",
        ),
        Ok(out) => Finding::new(
            "requirements",
            Level::Error,
            format!(
                "the machine doesn't meet the runtime's requirements ({})",
                out.describe_failure(timeout)
            ),
        ),
        Err(e) => Finding::new(
            "requirements",
            Level::Error,
            format!("can't run {CHECK_REQUIREMENTS}: {e}"),
        ),
    }
}

/// `steam-runtime-system-info`'s report.
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// The JSON as printed, for `-D --json` and bug reports.
    pub raw: String,
    pub value: Value,
}

/// Why system-info gave no report.
#[derive(Debug, thiserror::Error)]
pub enum DiagError {
    #[error("can't run {SYSTEM_INFO}: {0}")]
    Io(#[from] io::Error),
    #[error("{SYSTEM_INFO} failed ({0})")]
    Failed(String),
    #[error("{SYSTEM_INFO} printed something that isn't JSON: {0}")]
    BadOutput(#[from] serde_json::Error),
}

/// Runs `steam-runtime-system-info`.
pub fn system_info(tools: &Tools, timeout: Duration) -> Result<SystemInfo, DiagError> {
    let out = run_tool(&tools.argv(SYSTEM_INFO, &[]), timeout)?;
    if !out.success() {
        return Err(DiagError::Failed(out.describe_failure(timeout)));
    }
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    let value = serde_json::from_str(&raw)?;
    Ok(SystemInfo { raw, value })
}

impl SystemInfo {
    /// The parts of the report that decide whether games run.
    pub fn findings(&self) -> Vec<Finding> {
        summarize(&self.value)
    }
}

/// See [`SystemInfo::findings`].
pub fn summarize(info: &Value) -> Vec<Finding> {
    let mut out = Vec::new();

    let container = &info["container"];
    match container["bubblewrap_ok"].as_bool() {
        Some(false) => out.push(
            Finding::new(
                "system-info",
                Level::Error,
                "bubblewrap doesn't work, so container runtimes can't start",
            )
            .hint("see the kernel and bwrap findings"),
        ),
        Some(true) => out.push(Finding::new("system-info", Level::Ok, "bubblewrap works")),
        None => {}
    }
    if let Some(kind) = container["type"].as_str()
        && kind != "none"
    {
        out.push(
            Finding::new(
                "system-info",
                Level::Warn,
                format!("sruman itself runs inside a container ({kind})"),
            )
            .hint("pressure-vessel can't nest inside most containers; run sruman on the host"),
        );
    }

    if let Some(arches) = info["architectures"].as_object() {
        for (arch, details) in arches {
            if details["can-run"].as_bool() != Some(false) {
                continue;
            }
            let finding = if arch.starts_with("i386") {
                Finding::new(
                    "system-info",
                    Level::Warn,
                    format!("{arch} programs can't run: the host has no 32-bit libraries"),
                )
                .hint("only 32-bit games need them; install your distribution's 32-bit glibc (lib32-glibc, libc6:i386) for those")
            } else {
                Finding::new(
                    "system-info",
                    Level::Error,
                    format!("{arch} programs can't run"),
                )
            };
            out.push(finding);
        }
    }

    if let Some(features) = info["cpu-features"].as_object() {
        let missing: Vec<&str> = features
            .iter()
            .filter(|(_, v)| v.as_bool() == Some(false))
            .map(|(k, _)| k.as_str())
            .collect();
        if !missing.is_empty() {
            out.push(Finding::new(
                "system-info",
                Level::Error,
                format!(
                    "the CPU lacks features the runtime needs: {}",
                    missing.join(", ")
                ),
            ));
        }
    }

    if info["locales-ok"].as_bool() == Some(false) {
        out.push(
            Finding::new(
                "system-info",
                Level::Warn,
                "the host's locale settings are broken",
            )
            .hint("generate the locale named in $LANG (locale-gen), or unset it"),
        );
    }
    out
}

#[cfg(test)]
mod tests;
