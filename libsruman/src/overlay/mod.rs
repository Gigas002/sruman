//! Overlay mode (D13, D14): a game runs on the host, with a runtime's
//! libraries offered through `LD_LIBRARY_PATH`.
//!
//! sruman builds, per runtime build and set of choices, a folder of
//! symlinks named like the libraries (`libSDL2-2.0.so.0 -> <build>/…`), one
//! subfolder per ABI, and puts those first on `LD_LIBRARY_PATH`. A library
//! that isn't linked there is found by the loader on the host as usual.
//!
//! Which copy of a library a game gets:
//!
//! - glibc and the GPU stack (GL/EGL/Vulkan loaders, `libdrm`, `libgbm`,
//!   VA-API, VDPAU, NVIDIA's libraries) always come from the host: the host's
//!   dynamic loader can't use an older libc, and the drivers need the host's
//!   own libraries ([`host_only`]).
//! - Everything else is `newest` unless `[slr.lib_overrides]` says
//!   otherwise: the runtime's copy when the host has none or an older one.
//!   Offering an older copy first breaks the host's drivers, which load
//!   with the game: with steamrt4's `libwayland-client` first, Arch's RADV
//!   fails with `undefined symbol: wl_display_dispatch_queue_timeout`.
//!   Steam's own LD_LIBRARY_PATH runtime (scout's `run.sh`) does the same:
//!   host libraries first, runtime libraries only where they are newer.
//!
//! Valve's archives don't contain the library symlinks: the runtime's mtree
//! (`usr-mtree.txt.gz`) lists them, and pressure-vessel recreates them. The
//! overlay reads the same list.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use crate::{
    catalog::{Arch, Entry},
    model::LibOverride,
    mtree::{self, Kind, MtreeError},
};

#[cfg(test)]
pub(crate) mod tests;

/// Errors from setting up overlay mode.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// The build has no platform directory (`*_platform_*`) to take libraries from.
    #[error("{0} has no `*_platform_*` directory")]
    NoPlatform(PathBuf),
    /// The build's file list can't be read.
    #[error(transparent)]
    Mtree(#[from] MtreeError),
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory it failed on.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// `ldconfig -p` failed or printed nothing usable (its error).
    #[error("can't list the host's libraries (`ldconfig -p`): {0}")]
    Ldconfig(String),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> OverlayError + '_ {
    move |source| OverlayError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A library ABI, as Debian names its directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Abi {
    /// 64-bit x86.
    X86_64,
    /// 32-bit x86.
    I386,
    /// 64-bit ARM.
    Aarch64,
}

impl Abi {
    /// Debian's multiarch triplet: `x86_64-linux-gnu`, `i386-linux-gnu`,
    /// `aarch64-linux-gnu`.
    pub fn triplet(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-linux-gnu",
            Self::I386 => "i386-linux-gnu",
            Self::Aarch64 => "aarch64-linux-gnu",
        }
    }

    /// The ABIs a game on `arch` can use, main one first.
    pub fn for_arch(arch: Arch) -> &'static [Self] {
        match arch {
            Arch::X86_64 => &[Self::X86_64, Self::I386],
            Arch::Arm64 => &[Self::Aarch64],
        }
    }

    /// From the flags `ldconfig -p` prints: `libc6,x86-64`, `libc6`,
    /// `libc6,AArch64`. `x32` and other ABIs are `None`.
    fn from_ldconfig(flags: &str) -> Option<Self> {
        if flags.contains("x86-64") {
            Some(Self::X86_64)
        } else if flags.contains("AArch64") {
            Some(Self::Aarch64)
        } else if flags.starts_with("libc6") && !flags.contains("x32") && !flags.contains("64") {
            Some(Self::I386)
        } else {
            None
        }
    }
}

impl fmt::Display for Abi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.triplet())
    }
}

// --------------------------------------------------------------- the host

/// The host's libraries, as the dynamic loader finds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostLibs {
    libs: BTreeMap<(Abi, String), PathBuf>,
}

impl HostLibs {
    /// Runs `ldconfig -p`.
    #[expect(
        clippy::disallowed_methods,
        reason = "`ldconfig -p` lists the host's libraries for overlay mode; fixed argv, no shell"
    )]
    pub fn system() -> Result<Self, OverlayError> {
        let mut last = String::from("not found");
        for program in [
            "ldconfig",
            "/usr/bin/ldconfig",
            "/sbin/ldconfig",
            "/usr/sbin/ldconfig",
        ] {
            match Command::new(program).arg("-p").output() {
                Ok(out) if out.status.success() => {
                    return Ok(Self::parse(&String::from_utf8_lossy(&out.stdout)));
                }
                Ok(out) => last = out.status.to_string(),
                Err(e) => last = e.to_string(),
            }
        }
        Err(OverlayError::Ldconfig(last))
    }

    /// Parses `ldconfig -p` output. The first entry of a name wins, as for
    /// the loader.
    pub fn parse(text: &str) -> Self {
        let mut libs = BTreeMap::new();
        for line in text.lines() {
            let Some((left, path)) = line.split_once(" => ") else {
                continue;
            };
            let Some((name, flags)) = left.trim().split_once(" (") else {
                continue;
            };
            let Some(abi) = Abi::from_ldconfig(flags.trim_end_matches(')')) else {
                continue;
            };
            libs.entry((abi, name.to_owned()))
                .or_insert_with(|| PathBuf::from(path.trim()));
        }
        Self { libs }
    }

    /// The host's file for library `name` of `abi`.
    pub fn get(&self, abi: Abi, name: &str) -> Option<&Path> {
        self.libs.get(&(abi, name.to_owned())).map(PathBuf::as_path)
    }
}

// ------------------------------------------------------------ the runtime

/// One library name a runtime build offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLib {
    /// Its ABI.
    pub abi: Abi,
    /// The name the loader looks for: `libSDL2-2.0.so.0`.
    pub name: String,
    /// The file holding it, absolute.
    pub file: PathBuf,
    /// The name of the real file (`libSDL2-2.0.so.0.3200.74`), for versions.
    pub real_name: String,
}

/// Where a build keeps its libraries: the root its mtree describes, the
/// mtree (if any), and the library directories under the root, in search
/// order.
struct Layout {
    root: PathBuf,
    mtree: Option<PathBuf>,
    dirs: Vec<String>,
}

fn layout(build_dir: &Path, entry: Entry, abi: Abi) -> Result<Layout, OverlayError> {
    let t = abi.triplet();
    match entry {
        Entry::Container => {
            let platform = platform_dir(build_dir)
                .ok_or_else(|| OverlayError::NoPlatform(build_dir.to_path_buf()))?;
            Ok(Layout {
                root: platform.join("files"),
                mtree: Some(platform.join("usr-mtree.txt.gz")).filter(|p| p.is_file()),
                dirs: vec![format!("lib/{t}")],
            })
        }
        // The LD_LIBRARY_PATH runtime the scout archive bundles, in the
        // order its `run.sh` uses.
        Entry::ScoutOnSoldier => Ok(Layout {
            root: build_dir.to_path_buf(),
            mtree: Some(build_dir.join("mtree.txt.gz")).filter(|p| p.is_file()),
            dirs: vec![
                format!("steam-runtime/lib/{t}"),
                format!("steam-runtime/usr/lib/{t}"),
            ],
        }),
    }
}

/// The newest `<suite>_platform_<build>/` of a container build.
pub fn platform_dir(build_dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(build_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains("_platform_"))
        })
        .collect();
    found.sort_by_key(|path| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let build = name.split_once("_platform_").map(|(_, b)| b.to_owned());
        build.and_then(|b| b.parse::<crate::version::BuildId>().ok())
    });
    found.pop()
}

/// Whether a directory entry is something the loader opens by name:
/// `libz.so.1`, or a real file named `*.so` (`libpulsecommon-14.2.so`, whose
/// soname has no number). Unversioned links (`libz.so`) are for linking only.
fn is_library_name(name: &str, is_file: bool) -> bool {
    name.starts_with("lib") && (name.contains(".so.") || (is_file && name.ends_with(".so")))
        || name.starts_with("ld-linux")
}

/// Every library `build_dir` offers for `abis`.
pub fn runtime_libs(
    build_dir: &Path,
    entry: Entry,
    abis: &[Abi],
) -> Result<Vec<RuntimeLib>, OverlayError> {
    let mut out = Vec::new();
    let mut mtrees: BTreeMap<PathBuf, Vec<mtree::Entry>> = BTreeMap::new();
    for &abi in abis {
        let layout = layout(build_dir, entry, abi)?;
        let mut seen = BTreeSet::new();
        match &layout.mtree {
            Some(path) => {
                if !mtrees.contains_key(path) {
                    mtrees.insert(path.clone(), mtree::read_gz(path)?);
                }
                from_mtree(&mtrees[path], &layout, abi, &mut seen, &mut out);
            }
            None => from_fs(&layout, abi, &mut seen, &mut out)?,
        }
    }
    Ok(out)
}

fn from_mtree(
    entries: &[mtree::Entry],
    layout: &Layout,
    abi: Abi,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<RuntimeLib>,
) {
    let by_path: BTreeMap<&Path, &mtree::Entry> =
        entries.iter().map(|e| (e.path.as_path(), e)).collect();
    for dir in &layout.dirs {
        let in_dir = |e: &&mtree::Entry| e.path.parent() == Some(Path::new(dir));
        // Real files a link names (`libz.so.1.3.1` behind `libz.so.1`) are
        // offered under the link's name only.
        let named: BTreeSet<PathBuf> = entries
            .iter()
            .filter(in_dir)
            .filter(|e| e.kind == Kind::Link)
            .filter_map(|e| normalize(&e.path.parent()?.join(e.link.as_ref()?)))
            .collect();
        for entry in entries.iter().filter(in_dir) {
            if entry.kind == Kind::Dir || (entry.kind == Kind::File && named.contains(&entry.path))
            {
                continue;
            }
            let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !is_library_name(name, entry.kind == Kind::File) || seen.contains(name) {
                continue;
            }
            let Some(real) = resolve(&by_path, entry) else {
                continue;
            };
            let file = layout.root.join(real.data_path());
            if !file.is_file() {
                continue;
            }
            let real_name = real
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            seen.insert(name.to_owned());
            out.push(RuntimeLib {
                abi,
                name: name.to_owned(),
                file,
                real_name,
            });
        }
    }
}

/// Follows mtree links (relative, within the tree) to a file entry.
fn resolve<'a>(
    by_path: &BTreeMap<&Path, &'a mtree::Entry>,
    mut entry: &'a mtree::Entry,
) -> Option<&'a mtree::Entry> {
    for _ in 0..16 {
        match entry.kind {
            Kind::File => return Some(entry),
            Kind::Dir => return None,
            Kind::Link => {
                let link = entry.link.as_ref()?;
                let target = normalize(&entry.path.parent()?.join(link))?;
                entry = by_path.get(target.as_path())?;
            }
        }
    }
    None
}

/// `a/b/../c` → `a/c`; `None` if it leaves the tree or is absolute.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn from_fs(
    layout: &Layout,
    abi: Abi,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<RuntimeLib>,
) -> Result<(), OverlayError> {
    for dir in &layout.dirs {
        let dir = layout.root.join(dir);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&dir)(e)),
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        // As in the mtree: files behind a link in the same directory are
        // offered under the link's name only.
        let named: BTreeSet<String> = names
            .iter()
            .filter_map(|name| fs::read_link(dir.join(name)).ok())
            .filter(|target| target.components().count() == 1)
            .map(|target| target.to_string_lossy().into_owned())
            .collect();
        for name in names {
            let path = dir.join(&name);
            let is_file = path.symlink_metadata().is_ok_and(|m| m.is_file());
            if !is_library_name(&name, is_file)
                || seen.contains(&name)
                || (is_file && named.contains(&name))
            {
                continue;
            }
            let Ok(real) = fs::canonicalize(&path) else {
                continue;
            };
            if !real.is_file() {
                continue;
            }
            let real_name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            seen.insert(name.clone());
            out.push(RuntimeLib {
                abi,
                name,
                file: real,
                real_name,
            });
        }
    }
    Ok(())
}

// --------------------------------------------------------------- choosing

/// The part of a library name before `.so`: `libSDL2-2.0`, `ld-linux-x86-64`.
fn stem(name: &str) -> &str {
    name.find(".so").map_or(name, |i| &name[..i])
}

/// glibc's libraries: always the host's.
pub fn is_glibc(name: &str) -> bool {
    let stem = stem(name);
    stem.starts_with("ld-linux")
        || stem.starts_with("libnss_")
        || matches!(
            stem,
            "libc"
                | "libm"
                | "libmvec"
                | "libpthread"
                | "libdl"
                | "librt"
                | "libutil"
                | "libresolv"
                | "libanl"
                | "libnsl"
                | "libBrokenLocale"
                | "libthread_db"
                | "libc_malloc_debug"
                | "libmemusage"
                | "libpcprofile"
        )
}

/// The GPU stack: loaders and driver libraries, always the host's.
pub fn is_gpu(name: &str) -> bool {
    let stem = stem(name);
    matches!(
        stem,
        "libGL"
            | "libGLX"
            | "libEGL"
            | "libGLESv1_CM"
            | "libGLESv2"
            | "libOpenGL"
            | "libGLdispatch"
            | "libvulkan"
            | "libgbm"
            | "libdrm"
            | "libglapi"
            | "libva"
            | "libvdpau"
            | "libxatracker"
            | "libcuda"
    ) || [
        "libGLX_",
        "libEGL_",
        "libdrm_",
        "libva-",
        "libvdpau_",
        "libnvidia-",
        "libgallium-",
    ]
    .iter()
    .any(|prefix| stem.starts_with(prefix))
}

/// A library only the host's copy can be used for.
pub fn host_only(name: &str) -> bool {
    is_glibc(name) || is_gpu(name)
}

/// The version numbers in a library's real file name, after its stem:
/// `libSDL2-2.0.so.0.3200.74` → `[0, 3200, 74]`, `libelf-0.192.so` →
/// `[0, 192]`.
fn file_version(name: &str, real_name: &str) -> Vec<u64> {
    let stem = stem(name);
    let rest = real_name.strip_prefix(stem).unwrap_or(real_name);
    rest.split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// The highest `<prefix>N.N…` version node in an ELF file
/// (`GLIBC_2.41`, `GCC_14.0.0`): what the library provides.
pub fn max_version_node(file: &Path, prefix: &str) -> Option<Vec<u64>> {
    let bytes = fs::read(file).ok()?;
    let needle = prefix.as_bytes();
    let mut best: Option<Vec<u64>> = None;
    let mut at = 0;
    while let Some(pos) = find(&bytes[at..], needle) {
        let start = at + pos + needle.len();
        let end = bytes[start..]
            .iter()
            .position(|b| !(b.is_ascii_digit() || *b == b'.'))
            .map_or(bytes.len(), |n| start + n);
        let text = String::from_utf8_lossy(&bytes[start..end]);
        let version: Vec<u64> = text
            .split('.')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect();
        if !version.is_empty() && best.as_ref().is_none_or(|b| version > *b) {
            best = Some(version);
        }
        at = start;
    }
    best
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Whether the runtime's copy is newer than the host's. Ties go to the
/// host. `libgcc_s.so.1` has no version in its file name, so its `GCC_`
/// version nodes decide.
fn runtime_is_newer(lib: &RuntimeLib, host: &Path) -> bool {
    let host_real = fs::canonicalize(host).unwrap_or_else(|_| host.to_path_buf());
    let host_name = host_real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ours = file_version(&lib.name, &lib.real_name);
    let theirs = file_version(&lib.name, &host_name);
    if ours != theirs {
        return ours > theirs;
    }
    if stem(&lib.name) == "libgcc_s" {
        return max_version_node(&lib.file, "GCC_") > max_version_node(&host_real, "GCC_");
    }
    false
}

/// Where a library a game asks for comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// From the runtime (the file).
    Runtime(PathBuf),
    /// From the host (the file).
    Host(PathBuf),
    /// Neither has it.
    Missing,
}

impl fmt::Display for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(path) => write!(f, "runtime ({})", path.display()),
            Self::Host(path) => write!(f, "host ({})", path.display()),
            Self::Missing => f.write_str("missing"),
        }
    }
}

/// What a `[slr.lib_overrides]` entry resolves to, per ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideReport {
    /// The library name.
    pub name: String,
    /// The ABI.
    pub abi: Abi,
    /// The `[slr.lib_overrides]` setting.
    pub setting: LibOverride,
    /// Where the library comes from with that setting.
    pub resolved: Resolved,
}

/// A planned overlay folder: what it links, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlan {
    /// The final folder (`<store>/.overlays/<name>/<build>/<hash>`).
    pub dir: PathBuf,
    /// Per ABI: name → file.
    pub links: BTreeMap<Abi, BTreeMap<String, PathBuf>>,
    /// The ABIs with a subfolder, main one first.
    pub abis: Vec<Abi>,
    /// Every `[slr.lib_overrides]` entry, resolved.
    pub overrides: Vec<OverrideReport>,
}

impl OverlayPlan {
    /// `<dir>/<triplet>` for each ABI, `:`-joined, main ABI first.
    pub fn ld_library_path(&self) -> OsString {
        let mut joined = OsString::new();
        for (i, abi) in self.abis.iter().enumerate() {
            if i > 0 {
                joined.push(":");
            }
            joined.push(self.dir.join(abi.triplet()));
        }
        joined
    }

    /// Creates the folder unless it already exists. Built next to its final
    /// place and renamed, so a game never sees half of one; when another
    /// launch wins the race, its folder is used (same contents).
    pub fn build(&self) -> Result<(), OverlayError> {
        if self.dir.is_dir() {
            return Ok(());
        }
        let parent = self.dir.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(io_err(parent))?;
        static N: AtomicU64 = AtomicU64::new(0);
        let tmp = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let result = self.fill(&tmp).and_then(|()| {
            match fs::rename(&tmp, &self.dir) {
                Ok(()) => Ok(()),
                // Another launch made it first.
                Err(_) if self.dir.is_dir() => Ok(()),
                Err(e) => Err(io_err(&self.dir)(e)),
            }
        });
        if tmp.exists() {
            let _ = fs::remove_dir_all(&tmp);
        }
        result
    }

    fn fill(&self, tmp: &Path) -> Result<(), OverlayError> {
        for abi in &self.abis {
            let dir = tmp.join(abi.triplet());
            fs::create_dir_all(&dir).map_err(io_err(&dir))?;
            for (name, file) in self.links.get(abi).into_iter().flatten() {
                let link = dir.join(name);
                std::os::unix::fs::symlink(file, &link).map_err(io_err(&link))?;
            }
        }
        Ok(())
    }
}

/// Decides every library: glibc and GPU from the host, overrides as set,
/// the rest `newest`. `base` is the build's overlay directory
/// (`Store::overlay_dir`); the folder inside it is named by a hash of the
/// links, so a host upgrade, a new build or other overrides get their own.
pub fn plan(
    libs: &[RuntimeLib],
    host: &HostLibs,
    overrides: &BTreeMap<String, LibOverride>,
    abis: &[Abi],
    base: &Path,
) -> OverlayPlan {
    let mut links: BTreeMap<Abi, BTreeMap<String, PathBuf>> = BTreeMap::new();
    for lib in libs {
        if host_only(&lib.name) {
            continue;
        }
        let host_copy = host.get(lib.abi, &lib.name);
        let link = match overrides
            .get(&lib.name)
            .copied()
            .unwrap_or(LibOverride::Newest)
        {
            LibOverride::Runtime => true,
            LibOverride::Host => false,
            LibOverride::Newest => host_copy.is_none_or(|h| runtime_is_newer(lib, h)),
        };
        if link {
            links
                .entry(lib.abi)
                .or_default()
                .insert(lib.name.clone(), lib.file.clone());
        }
    }

    let mut reports = Vec::new();
    for (name, &setting) in overrides {
        for &abi in abis {
            let resolved = match links.get(&abi).and_then(|l| l.get(name)) {
                Some(file) => Resolved::Runtime(file.clone()),
                None => host
                    .get(abi, name)
                    .map_or(Resolved::Missing, |p| Resolved::Host(p.to_path_buf())),
            };
            reports.push(OverrideReport {
                name: name.clone(),
                abi,
                setting,
                resolved,
            });
        }
    }

    let mut hasher = Sha256::new();
    for (abi, names) in &links {
        for (name, file) in names {
            hasher.update(abi.triplet().as_bytes());
            hasher.update([0]);
            hasher.update(name.as_bytes());
            hasher.update([0]);
            hasher.update(file.as_os_str().as_encoded_bytes());
            hasher.update(b"\n");
        }
    }
    let hash = mtree::hex(&hasher.finalize()[..8]);
    OverlayPlan {
        dir: base.join(hash),
        links,
        abis: abis.to_vec(),
        overrides: reports,
    }
}

/// The glibc version the runtime's libraries were built for, against the
/// host's: an error naming both when the host's is older. `None` when
/// either has no libc to compare (scout ships none).
pub fn glibc_problem(libs: &[RuntimeLib], host: &HostLibs, abi: Abi) -> Option<String> {
    let ours = libs
        .iter()
        .find(|l| l.abi == abi && l.name == "libc.so.6")?;
    let theirs = host.get(abi, "libc.so.6")?;
    let ours_v = max_version_node(&ours.file, "GLIBC_")?;
    let theirs_v = max_version_node(theirs, "GLIBC_")?;
    let show = |v: &[u64]| v.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
    (theirs_v < ours_v).then(|| {
        format!(
            "overlay mode needs the host's glibc to be at least the runtime's: \
             the runtime has {}, the host {}; use `mode = \"container\"`",
            show(&ours_v),
            show(&theirs_v)
        )
    })
}
