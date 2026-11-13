//! Turns a game, an executable or a shell plus a profile into a command
//! that runs it inside a Steam Linux Runtime.
//!
//! Two steps, so nothing is touched until a launch is certain:
//!
//! 1. [`prepare`] resolves everything (runtime builds, executable, working
//!    directory, wrappers, env) and collects every problem at once. It only
//!    reads the filesystem, so `--print`, `-Pk` and tests use it freely.
//! 2. [`LaunchPlan::acquire`] takes a shared hold on each runtime build and
//!    sets up the prefix (its shared lock included), and returns the final
//!    [`LaunchSpec`]. The caller spawns it, keeping [`Launch::fds`] open in
//!    the child (no `CLOEXEC`): pressure-vessel closes inherited fds, so
//!    they're handed to the game with `--pass-fd`, and the locks last
//!    exactly as long as the game.
//!
//! [`wrap`] is the part lootrs and gaki use: a finished native command in,
//! the same command inside a runtime out.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::fs::PermissionsExt,
    },
    path::{Path, PathBuf},
};

use crate::{
    catalog::{Catalog, Entry},
    model::{Game, Issue, Issues, Profile, ShaderCache, SlrSection, Subject, ValidationContext},
    paths,
    prefix::{self, HomeMode, PrefixContext, PrefixUse},
    store::{Installed, Store},
    version::RuntimeRef,
};

#[cfg(test)]
mod tests;

/// Set to the game's name on every process sruman starts for a game.
pub const TAG_GAME: &str = "SRUMAN_GAME";
/// Set on path runs and shells: `<kind>:<profile>:<id>`.
pub const TAG_SESSION: &str = "SRUMAN_SESSION";
pub const TAG_PROFILE: &str = "SRUMAN_PROFILE";
/// The absolute prefix path, when the profile has one.
pub const TAG_PREFIX: &str = "SRUMAN_PREFIX";

/// What to launch.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Game(&'a Game),
    /// An executable: a path (`~` allowed), or a bare name looked up in the
    /// runtime's `PATH`.
    Exe(&'a Path),
    /// An interactive shell in the runtime (`-X --shell`).
    Shell,
}

/// How the processes are tagged, see [`TAG_GAME`] and [`TAG_SESSION`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Game(String),
    Session(String),
}

impl Tag {
    fn env(&self) -> (&'static str, OsString) {
        match self {
            Self::Game(name) => (TAG_GAME, name.into()),
            Self::Session(session) => (TAG_SESSION, session.into()),
        }
    }
}

/// Values that come from the caller, not the profile.
#[derive(Debug, Clone, Copy)]
pub struct LaunchContext<'a> {
    pub home: &'a Path,
    /// Directories from the host `PATH`, for wrappers.
    pub search_path: &'a [PathBuf],
    /// `--debug` on top of the profile's `debug`.
    pub debug: bool,
    /// Appended after the game's own arguments.
    pub extra_args: &'a [String],
    pub store: &'a Store,
    pub catalog: &'a Catalog,
}

/// A resolved command, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Set on top of the inherited environment.
    pub env: BTreeMap<String, OsString>,
    pub cwd: PathBuf,
    /// The game's shader cache directory, which `env` points the drivers
    /// at; created by [`LaunchPlan::acquire`].
    pub shader_cache: Option<PathBuf>,
}

impl LaunchSpec {
    /// Program and arguments.
    pub fn argv(&self) -> impl Iterator<Item = &std::ffi::OsStr> {
        std::iter::once(self.program.as_os_str()).chain(self.args.iter().map(OsString::as_os_str))
    }
}

/// Every problem that prevents a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchError {
    pub problems: Vec<String>,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.problems.join("\n"))
    }
}

impl std::error::Error for LaunchError {}

/// A finished native command, for [`wrap`]: what would run without a
/// runtime. Wrappers (`mangohud`, …) stay outside: the caller puts them in
/// front of the wrapped command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, OsString>,
    pub cwd: PathBuf,
}

/// The runtime builds a launch uses.
#[derive(Debug, Clone)]
pub struct Runtimes {
    /// The profile's runtime.
    pub runtime: Installed,
    /// The container it runs in, for scout: soldier's current build.
    pub host: Option<Installed>,
    pub entry: Entry,
}

/// The result of [`prepare`]: everything decided, nothing locked yet.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Wrapper argv (resolved on the host), in front of everything.
    wrappers: Vec<OsString>,
    /// The runtime entry point and its options, up to (not including) the
    /// `--pass-fd` options and the `--` before the command.
    entry: Vec<OsString>,
    /// After `--pass-fd`: `--`, the command and its arguments.
    command: Vec<OsString>,
    /// Whether fds have to be passed with `--pass-fd` (container runtimes).
    pass_fds: bool,
    env: BTreeMap<String, OsString>,
    cwd: PathBuf,
    shader_cache: Option<PathBuf>,
    /// Builds to hold while running.
    pub builds: Vec<Installed>,
    /// The prefix to set up, and how.
    pub prefix: Option<PrefixPlan>,
}

/// What [`LaunchPlan::acquire`] does to the prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixPlan {
    /// Absolute.
    pub dir: PathBuf,
    pub runtime: String,
    pub build: String,
    pub profile: String,
    pub home: HomeMode,
}

/// A launch with its locks held.
#[derive(Debug)]
pub struct Launch {
    pub spec: LaunchSpec,
    /// Keep these open in the child (clear `CLOEXEC` before exec): the build
    /// holds and the prefix lock.
    pub fds: Vec<OwnedFd>,
}

impl Launch {
    /// Clears `CLOEXEC` on [`Launch::fds`] so the next spawned child keeps
    /// them. Call it right before spawning the game.
    pub fn make_inheritable(&self) -> std::io::Result<()> {
        for fd in &self.fds {
            let flags = rustix::io::fcntl_getfd(fd)?;
            rustix::io::fcntl_setfd(fd, flags - rustix::io::FdFlags::CLOEXEC)?;
        }
        Ok(())
    }
}

impl LaunchPlan {
    /// The spec as it would run with `fds` passed (none for a preview).
    pub fn spec(&self, fds: &[i32]) -> LaunchSpec {
        let mut argv: Vec<OsString> = self.wrappers.clone();
        argv.extend(self.entry.iter().cloned());
        if self.pass_fds {
            argv.extend(
                fds.iter()
                    .map(|fd| OsString::from(format!("--pass-fd={fd}"))),
            );
        }
        argv.extend(self.command.iter().cloned());
        let mut argv = argv.into_iter();
        let program = PathBuf::from(argv.next().expect("an entry point"));
        LaunchSpec {
            program,
            args: argv.collect(),
            env: self.env.clone(),
            cwd: self.cwd.clone(),
            shader_cache: self.shader_cache.clone(),
        }
    }

    /// Holds every build, sets up the prefix, creates the shader cache
    /// directory, and returns the spec to spawn. Fails with the problems
    /// (a busy prefix, a build being removed) without leaving anything
    /// locked.
    pub fn acquire(
        &self,
        store: &Store,
        prefix_ctx: &PrefixContext,
    ) -> Result<Launch, LaunchError> {
        let fail = |message: String| LaunchError {
            problems: vec![message],
        };
        let mut fds: Vec<OwnedFd> = Vec::new();
        for build in &self.builds {
            let hold = store
                .hold(build)
                .map_err(|e| fail(format!("{} {}: {e}", build.name, build.build)))?;
            fds.push(hold.into_fd());
        }
        if let Some(plan) = &self.prefix {
            let prepared = prefix::ensure(
                &plan.dir,
                &PrefixUse {
                    runtime: &plan.runtime,
                    build: &plan.build,
                    profile: &plan.profile,
                    home: plan.home,
                },
                prefix_ctx,
            )
            .map_err(|e| fail(e.to_string()))?;
            fds.push(prepared.lock.into_fd());
        }
        if let Some(dir) = &self.shader_cache {
            std::fs::create_dir_all(dir)
                .map_err(|e| fail(format!("creating {}: {e}", dir.display())))?;
        }
        let raw: Vec<i32> = fds.iter().map(AsRawFd::as_raw_fd).collect();
        Ok(Launch {
            spec: self.spec(&raw),
            fds,
        })
    }
}

/// Finds an executable: paths (with a `/`) are checked directly, bare names
/// in `search_path`.
pub fn find_program(program: &Path, search_path: &[PathBuf]) -> Option<PathBuf> {
    if program.components().count() > 1 || program.is_absolute() {
        return is_executable(program).then(|| program.to_path_buf());
    }
    search_path
        .iter()
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Each wrapper entry is split on whitespace (`"mangohud --dlsym"`), and
/// its program is found on the host.
fn resolve_wrappers(
    wrappers: &[String],
    search_path: &[PathBuf],
    problems: &mut Vec<String>,
) -> Vec<OsString> {
    let mut argv = Vec::new();
    for wrapper in wrappers {
        let mut words = wrapper.split_whitespace();
        let Some(program) = words.next() else {
            continue;
        };
        match find_program(Path::new(program), search_path) {
            Some(resolved) => argv.push(resolved.into_os_string()),
            None => problems.push(format!("wrapper `{program}` not found in PATH")),
        }
        argv.extend(words.map(OsString::from));
    }
    argv
}

/// The installed build for `runtime`, or a problem naming the fix.
fn resolve_build(
    runtime: &RuntimeRef,
    store: &Store,
    problems: &mut Vec<String>,
) -> Option<Installed> {
    match store.resolve(runtime) {
        Ok(installed) => Some(installed),
        Err(_) => {
            problems.push(format!(
                "runtime `{runtime}` is not installed; run `sruman -S {runtime}`"
            ));
            None
        }
    }
}

/// The runtime builds a profile's `[slr]` needs, or the problems.
pub fn resolve_runtimes(
    slr: &SlrSection,
    ctx: &LaunchContext,
    problems: &mut Vec<String>,
) -> Option<Runtimes> {
    let Some(def) = ctx.catalog.get(&slr.runtime.name) else {
        problems.push(format!("unknown runtime `{}`", slr.runtime.name));
        return None;
    };
    let runtime = resolve_build(&slr.runtime, ctx.store, problems)?;
    let marker = runtime.dir.join(def.entry.marker());
    if !is_executable(&marker) {
        problems.push(format!(
            "{} {} is incomplete: {} is missing; check it with `sruman -Qkk {}`",
            runtime.name,
            runtime.build,
            marker.display(),
            runtime.name
        ));
        return None;
    }
    let host = match def.entry {
        Entry::ScoutOnSoldier => {
            // `depends` names the container scout runs in (soldier).
            let Some(host_name) = def.depends.first() else {
                problems.push(format!(
                    "`{}` has no container runtime in `depends`",
                    def.name
                ));
                return None;
            };
            let host_ref = RuntimeRef {
                name: host_name.clone(),
                build: None,
            };
            let host = store_resolve_for(&host_ref, &def.name, ctx.store, problems)?;
            let run = host.dir.join(Entry::Container.marker());
            if !is_executable(&run) {
                problems.push(format!(
                    "{} {} is incomplete: {} is missing; check it with `sruman -Qkk {}`",
                    host.name,
                    host.build,
                    run.display(),
                    host.name
                ));
                return None;
            }
            Some(host)
        }
        Entry::Container | Entry::Ldlp => None,
    };
    Some(Runtimes {
        runtime,
        host,
        entry: def.entry,
    })
}

/// Like [`resolve_build`] for a dependency, naming who needs it.
fn store_resolve_for(
    runtime: &RuntimeRef,
    needed_by: &str,
    store: &Store,
    problems: &mut Vec<String>,
) -> Option<Installed> {
    match store.resolve(runtime) {
        Ok(installed) => Some(installed),
        Err(_) => {
            problems.push(format!(
                "`{needed_by}` runs inside `{runtime}`, which is not installed; run `sruman -S {runtime}`"
            ));
            None
        }
    }
}

/// Joins paths for `PRESSURE_VESSEL_FILESYSTEMS_*`, which splits on `:`.
fn join_paths(field: &str, list: &[PathBuf], problems: &mut Vec<String>) -> Option<OsString> {
    if list.is_empty() {
        return None;
    }
    let mut joined = OsString::new();
    for (i, path) in list.iter().enumerate() {
        if path.as_os_str().as_encoded_bytes().contains(&b':') {
            problems.push(format!(
                "{field}: {} contains `:`, which pressure-vessel can't share",
                path.display()
            ));
            continue;
        }
        if i > 0 {
            joined.push(":");
        }
        joined.push(path);
    }
    Some(joined)
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

/// Wraps `native` in the runtime: the entry point, its env and shared
/// paths. `prefix` (absolute) becomes the private home. Returns the plan
/// without wrappers; [`prepare`] adds them.
pub fn wrap(
    native: NativeCommand,
    slr: &SlrSection,
    profile: &Profile,
    prefix: Option<&Path>,
    ctx: &LaunchContext,
) -> Result<LaunchPlan, LaunchError> {
    let mut problems = Vec::new();
    let runtimes = resolve_runtimes(slr, ctx, &mut problems);
    let Some(runtimes) = runtimes else {
        return Err(LaunchError { problems });
    };
    let home = ctx.home;
    let debug = ctx.debug || profile.debug;
    let build = &runtimes.runtime;
    let prefix_plan = |mode| {
        prefix.map(|dir| PrefixPlan {
            dir: dir.to_path_buf(),
            runtime: build.name.clone(),
            build: build.build.to_string(),
            profile: profile.name.clone(),
            home: mode,
        })
    };

    if runtimes.entry == Entry::Ldlp {
        // No container: the host filesystem is all there, and the prefix
        // home can only be offered through the environment.
        let mut env = BTreeMap::new();
        if let Some(prefix) = prefix {
            let home_dir = prefix.join(prefix::HOME_DIR);
            for (key, sub) in [
                ("HOME", ""),
                ("XDG_CONFIG_HOME", ".config"),
                ("XDG_DATA_HOME", ".local/share"),
                ("XDG_CACHE_HOME", ".cache"),
                ("XDG_STATE_HOME", ".local/state"),
            ] {
                let dir = if sub.is_empty() {
                    home_dir.clone()
                } else {
                    home_dir.join(sub)
                };
                env.insert(key.to_owned(), dir.into_os_string());
            }
        }
        env.extend(native.env);
        let mut command = vec![native.program];
        command.extend(native.args);
        return Ok(LaunchPlan {
            wrappers: Vec::new(),
            entry: vec![build.dir.join(Entry::Ldlp.marker()).into_os_string()],
            command,
            // Nothing in between closes fds: the holds are inherited.
            pass_fds: false,
            env,
            cwd: native.cwd,
            shader_cache: None,
            builds: vec![runtimes.runtime.clone()],
            prefix: prefix_plan(HomeMode::Env),
        });
    }

    // The container: the runtime itself, or soldier for scout.
    let container = runtimes.host.as_ref().unwrap_or(build);
    let mut rw: Vec<PathBuf> = Vec::new();
    let mut ro: Vec<PathBuf> = Vec::new();
    let program = PathBuf::from(&native.program);
    if program.is_absolute()
        && let Some(dir) = program.parent()
    {
        push_unique(&mut rw, dir.to_path_buf());
    }
    push_unique(&mut rw, native.cwd.clone());
    if runtimes.entry == Entry::ScoutOnSoldier {
        // The scout entry point keeps its runtime copy in the scout build.
        push_unique(&mut rw, build.dir.clone());
    }
    for path in &slr.filesystems_rw {
        push_unique(&mut rw, paths::expand_tilde(path, home));
    }
    for path in &slr.filesystems_ro {
        push_unique(&mut ro, paths::expand_tilde(path, home));
    }
    if let Some(cache) = native.env.get("MESA_SHADER_CACHE_DIR") {
        push_unique(&mut rw, PathBuf::from(cache));
    }

    let mut env = BTreeMap::new();
    env.insert(
        "PRESSURE_VESSEL_VARIABLE_DIR".to_owned(),
        container.dir.join("var").into_os_string(),
    );
    if let Some(prefix) = prefix {
        env.insert(
            "PRESSURE_VESSEL_HOME".to_owned(),
            prefix.join(prefix::HOME_DIR).into_os_string(),
        );
    }
    if debug {
        env.insert("PRESSURE_VESSEL_VERBOSE".to_owned(), "1".into());
    }
    if let Some(value) = join_paths("filesystems_rw", &rw, &mut problems) {
        env.insert("PRESSURE_VESSEL_FILESYSTEMS_RW".to_owned(), value);
    }
    if let Some(value) = join_paths("filesystems_ro", &ro, &mut problems) {
        env.insert("PRESSURE_VESSEL_FILESYSTEMS_RO".to_owned(), value);
    }
    // The command's own env (profile, game, tags) over the runtime's.
    env.extend(native.env);
    if !problems.is_empty() {
        return Err(LaunchError { problems });
    }

    let mut entry = vec![
        container
            .dir
            .join(Entry::Container.marker())
            .into_os_string(),
    ];
    entry.extend(profile.runner_args.iter().map(OsString::from));
    let mut command = vec![OsString::from("--")];
    if runtimes.entry == Entry::ScoutOnSoldier {
        command.push(
            build
                .dir
                .join(Entry::ScoutOnSoldier.marker())
                .into_os_string(),
        );
        command.push("--".into());
    }
    command.push(native.program);
    command.extend(native.args);

    let mut builds = vec![runtimes.runtime.clone()];
    builds.extend(runtimes.host.clone());
    Ok(LaunchPlan {
        wrappers: Vec::new(),
        entry,
        command,
        pass_fds: true,
        env,
        cwd: native.cwd,
        shader_cache: None,
        builds,
        prefix: prefix_plan(HomeMode::Container),
    })
}

/// Resolves `target` under `profile` into a [`LaunchPlan`], collecting
/// every problem.
pub fn prepare(
    target: Target,
    profile: &Profile,
    tag: &Tag,
    ctx: &LaunchContext,
) -> Result<LaunchPlan, LaunchError> {
    let mut problems = Vec::new();
    let home = ctx.home;
    let prefix = profile
        .prefix
        .as_deref()
        .map(|p| paths::expand_tilde(p, home));

    let (program, mut args, cwd): (OsString, Vec<OsString>, PathBuf) = match target {
        Target::Game(game) => {
            let exe = paths::expand_tilde(&game.exe, home);
            check_exe(&exe, &mut problems);
            let cwd = match &game.cwd {
                Some(cwd) => paths::expand_tilde(cwd, home),
                None => exe
                    .parent()
                    .map_or_else(|| home.to_path_buf(), Path::to_path_buf),
            };
            let args = game.args.iter().map(OsString::from).collect();
            (exe.into_os_string(), args, cwd)
        }
        Target::Exe(path) => {
            let exe = paths::expand_tilde(path, home);
            if exe.components().count() > 1 || exe.is_absolute() {
                check_exe(&exe, &mut problems);
                let cwd = exe
                    .parent()
                    .map_or_else(|| home.to_path_buf(), Path::to_path_buf);
                (exe.into_os_string(), Vec::new(), cwd)
            } else {
                // A bare name: the runtime's PATH finds it.
                let cwd = prefix
                    .as_ref()
                    .map_or_else(|| home.to_path_buf(), |p| p.join(prefix::HOME_DIR));
                (exe.into_os_string(), Vec::new(), cwd)
            }
        }
        Target::Shell => {
            // In the prefix's home, so the real home isn't shared as the cwd.
            let cwd = prefix
                .as_ref()
                .map_or_else(|| home.to_path_buf(), |p| p.join(prefix::HOME_DIR));
            (OsString::from("bash"), Vec::new(), cwd)
        }
    };
    args.extend(ctx.extra_args.iter().map(OsString::from));
    // The prefix home is created by `acquire`; anything else must exist.
    let cwd_is_prefix_home = prefix
        .as_ref()
        .is_some_and(|p| cwd == p.join(prefix::HOME_DIR));
    if !cwd.is_dir() && !cwd_is_prefix_home {
        problems.push(format!("working directory not found: {}", cwd.display()));
    }
    let wrappers = match target {
        Target::Shell => Vec::new(),
        _ => resolve_wrappers(&profile.wrappers, ctx.search_path, &mut problems),
    };

    // Env, lowest first: shader cache < profile < game < tags.
    let mut env: BTreeMap<String, OsString> = BTreeMap::new();
    let shader_cache = match (target, &profile.shader_cache) {
        (Target::Game(game), Some(ShaderCache::Dir(dir))) => {
            let dir = paths::expand_tilde(dir, home).join(&game.name);
            for (key, value) in ShaderCache::env(&dir) {
                env.insert(key.to_owned(), value.into_os_string());
            }
            Some(dir)
        }
        _ => None,
    };
    env.extend(
        profile
            .env
            .iter()
            .map(|(k, v)| (k.clone(), OsString::from(v))),
    );
    if let Target::Game(game) = target {
        env.extend(game.env.iter().map(|(k, v)| (k.clone(), OsString::from(v))));
    }
    let (key, value) = tag.env();
    env.insert(key.to_owned(), value);
    env.insert(TAG_PROFILE.to_owned(), profile.name.clone().into());
    if let Some(prefix) = &prefix {
        env.insert(TAG_PREFIX.to_owned(), prefix.clone().into_os_string());
    }

    let native = NativeCommand {
        program,
        args,
        env,
        cwd,
    };
    let wrapped = wrap(native, &profile.slr, profile, prefix.as_deref(), ctx);
    match wrapped {
        Ok(mut plan) if problems.is_empty() => {
            plan.wrappers = wrappers;
            plan.shader_cache = shader_cache;
            Ok(plan)
        }
        Ok(_) => Err(LaunchError { problems }),
        Err(mut e) => {
            problems.append(&mut e.problems);
            Err(LaunchError { problems })
        }
    }
}

/// Checks what launching anything with `profile` needs, without launching
/// or locking: the profile's own validation, its runtime builds (scout's
/// soldier too), the wrappers, and the shared paths. Each problem names its
/// fix. Empty when everything is in place.
pub fn preflight(profile: &Profile, ctx: &LaunchContext) -> Vec<Issue> {
    let validation = ValidationContext {
        home: ctx.home,
        catalog: ctx.catalog,
    };
    let mut issues = profile.validate(&validation);
    if issues.iter().any(Issue::is_error) {
        return issues;
    }
    let mut collect = Issues::new(Subject::Profile(profile.name.clone()));
    let mut problems = Vec::new();
    resolve_runtimes(&profile.slr, ctx, &mut problems);
    resolve_wrappers(&profile.wrappers, ctx.search_path, &mut problems);
    for problem in problems {
        collect.error(problem);
    }
    for (field, paths) in [
        ("slr.filesystems_rw", &profile.slr.filesystems_rw),
        ("slr.filesystems_ro", &profile.slr.filesystems_ro),
    ] {
        for path in paths {
            let expanded = paths::expand_tilde(path, ctx.home);
            if !expanded.exists() {
                collect.warn(format!(
                    "`{field}`: {} doesn't exist, so it isn't shared",
                    expanded.display()
                ));
            }
        }
    }
    if let Some(prefix) = &profile.prefix {
        let prefix = paths::expand_tilde(prefix, ctx.home);
        if prefix.exists() && !prefix.is_dir() {
            collect.error(format!("`prefix` {} is not a directory", prefix.display()));
        }
    }
    issues.extend(collect.finish());
    issues
}

fn check_exe(exe: &Path, problems: &mut Vec<String>) {
    match std::fs::metadata(exe) {
        Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 => {}
        Ok(m) if m.is_file() => problems.push(format!(
            "{} is not executable; `chmod +x` it",
            exe.display()
        )),
        _ => problems.push(format!("executable not found: {}", exe.display())),
    }
}
