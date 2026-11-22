use std::{fs, os::unix::fs::PermissionsExt};

use super::*;
use crate::{
    model::new_profile,
    overlay::tests::{fake_host, fake_platform},
    store::{BuildMeta, MakeCurrent, Source},
    version::BuildId,
};

const BUILD: &str = "3.0.20261021.266025";

struct World {
    tmp: tempfile::TempDir,
    home: PathBuf,
    store: Store,
    catalog: Catalog,
    bin: PathBuf,
    /// The host's libraries for overlay mode: glibc 2.42, an old SDL.
    host: HostLibs,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::prefix::canonical(tmp.path());
        let home = root.join("home/u");
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for tool in ["gamemoderun", "mangohud"] {
            executable(&bin.join(tool));
        }
        let host = fake_host(
            &root.join("host"),
            &[
                (
                    "libc6,x86-64",
                    "libc.so.6",
                    "libc.so.6",
                    "GLIBC_2.2.5\0GLIBC_2.42\0",
                ),
                (
                    "libc6,x86-64",
                    "libSDL2-2.0.so.0",
                    "libSDL2-2.0.so.0.3200.10",
                    "",
                ),
                ("libc6,x86-64", "libz.so.1", "libz.so.1.3.1", ""),
            ],
        );
        let world = Self {
            store: Store::open(root.join("runtimes")),
            catalog: Catalog::builtin(),
            home,
            bin,
            host,
            tmp,
        };
        world.install("sniper", BUILD);
        world
    }

    /// The store lock. A child another test forks while this test has it
    /// open keeps it locked until the child's exec, so a busy lock is
    /// retried for a moment.
    fn lock(&self) -> crate::store::StoreLock {
        for _ in 0..200 {
            match self.store.lock() {
                Err(crate::store::StoreError::Busy(_)) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                other => return other.unwrap(),
            }
        }
        panic!("the store lock stayed busy");
    }

    fn install(&self, name: &str, build: &str) -> Installed {
        let lock = self.lock();
        let staged = self.store.staging_dir(&lock, name).unwrap();
        crate::store::tests::fake_container(&staged, name, build);
        let id: BuildId = build.parse().unwrap();
        let meta = BuildMeta::new(name, &id, Source::Import { from: "/x".into() });
        self.store
            .commit(&lock, &staged, meta, MakeCurrent::IfNewer)
            .unwrap()
    }

    fn ctx<'a>(&'a self, search: &'a [PathBuf], extra: &'a [String]) -> LaunchContext<'a> {
        LaunchContext {
            home: &self.home,
            search_path: search,
            debug: false,
            extra_args: extra,
            store: &self.store,
            catalog: &self.catalog,
            shell: Some(Path::new("/usr/bin/fish")),
            host_libs: Some(&self.host),
        }
    }

    fn game(&self) -> Game {
        let dir = self.home.join("Games/Buckshot");
        fs::create_dir_all(&dir).unwrap();
        executable(&dir.join("Buckshot Roulette.x86_64"));
        Game {
            name: "buckshot".into(),
            exe: "~/Games/Buckshot/Buckshot Roulette.x86_64".into(),
            profile: Some("indie".into()),
            args: vec!["--fullscreen".into()],
            cwd: None,
            env: BTreeMap::from([
                ("SDL_VIDEODRIVER".into(), "wayland".into()),
                ("A".into(), "game".into()),
            ]),
        }
    }

    fn build_dir(&self) -> PathBuf {
        self.store.resolve(&"sniper".parse().unwrap()).unwrap().dir
    }
}

fn executable(path: &Path) {
    fs::write(path, "#!/bin/sh\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn profile() -> Profile {
    let mut profile = new_profile("indie", &"sniper".parse().unwrap());
    profile.prefix = Some("~/prefixes/indie".into());
    profile.wrappers = vec!["gamemoderun".into(), "mangohud --dlsym".into()];
    profile.runner_args = vec!["--verbose".into()];
    profile.env = BTreeMap::from([
        ("A".into(), "profile".into()),
        ("B".into(), "profile".into()),
    ]);
    profile.shader_cache = Some(ShaderCache::Dir("~/.cache/shaders".into()));
    profile.slr.as_mut().unwrap().filesystems_rw =
        vec!["~/Music".into(), "~/Games/Buckshot".into()];
    profile.slr.as_mut().unwrap().filesystems_ro = vec!["/mnt/ro".into()];
    profile
}

fn strings(argv: impl Iterator<Item = impl AsRef<std::ffi::OsStr>>) -> Vec<String> {
    argv.map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect()
}

fn env_text(spec: &LaunchSpec) -> Vec<String> {
    spec.env
        .iter()
        .map(|(k, v)| format!("{k}={}", v.to_string_lossy()))
        .collect()
}

#[test]
fn game_in_a_container_with_a_prefix() {
    let w = World::new();
    let search = [w.bin.clone()];
    let extra = ["-x".to_owned()];
    let game = w.game();
    let plan = prepare(
        Target::Game(&game),
        &profile(),
        &Tag::Game("buckshot".into()),
        &w.ctx(&search, &extra),
    )
    .unwrap();
    let spec = plan.spec(&[3, 4]);
    let home = w.home.display().to_string();
    let build = w.build_dir().display().to_string();
    let bin = w.bin.display().to_string();
    assert_eq!(
        strings(spec.argv()),
        [
            format!("{bin}/gamemoderun"),
            format!("{bin}/mangohud"),
            "--dlsym".into(),
            format!("{build}/run"),
            "--verbose".into(),
            "--pass-fd=3".into(),
            "--pass-fd=4".into(),
            "--".into(),
            format!("{home}/Games/Buckshot/Buckshot Roulette.x86_64"),
            "--fullscreen".into(),
            "-x".into(),
        ]
    );
    assert_eq!(spec.cwd, w.home.join("Games/Buckshot"));
    assert_eq!(
        env_text(&spec),
        [
            "A=game".to_owned(),
            format!("AMD_VK_PIPELINE_CACHE_PATH={home}/.cache/shaders/buckshot/AMDv1"),
            "B=profile".into(),
            format!("MESA_SHADER_CACHE_DIR={home}/.cache/shaders/buckshot"),
            "PRESSURE_VESSEL_FILESYSTEMS_RO=/mnt/ro".into(),
            format!(
                "PRESSURE_VESSEL_FILESYSTEMS_RW={home}/Games/Buckshot:{home}/Music:{home}/.cache/shaders/buckshot"
            ),
            format!("PRESSURE_VESSEL_HOME={home}/prefixes/indie/home"),
            format!("PRESSURE_VESSEL_VARIABLE_DIR={build}/var"),
            "SDL_VIDEODRIVER=wayland".into(),
            "SRUMAN_GAME=buckshot".into(),
            format!("SRUMAN_PREFIX={home}/prefixes/indie"),
            "SRUMAN_PROFILE=indie".into(),
            format!("__GL_SHADER_DISK_CACHE_PATH={home}/.cache/shaders/buckshot/nvidiav1"),
        ]
    );
    assert_eq!(
        spec.shader_cache,
        Some(w.home.join(".cache/shaders/buckshot"))
    );
    assert_eq!(plan.builds.len(), 1);
    let prefix = plan.prefix.as_ref().unwrap();
    assert_eq!(prefix.home, HomeMode::Container);
    assert_eq!(prefix.build, BUILD);
}

#[test]
fn path_run_without_prefix_and_debug() {
    let w = World::new();
    let dir = w.home.join("Games/SuperTux");
    fs::create_dir_all(&dir).unwrap();
    executable(&dir.join("run.sh"));
    let mut profile = new_profile("sniper", &"sniper".parse().unwrap());
    profile.debug = true;
    let extra = ["--level".to_owned(), "2".to_owned()];
    let plan = prepare(
        Target::Exe(Path::new("~/Games/SuperTux/run.sh")),
        &profile,
        &Tag::Session("exec:sniper:1".into()),
        &w.ctx(&[], &extra),
    )
    .unwrap();
    let spec = plan.spec(&[]);
    let build = w.build_dir().display().to_string();
    assert_eq!(
        strings(spec.argv()),
        [
            format!("{build}/run"),
            "--".into(),
            dir.join("run.sh").display().to_string(),
            "--level".into(),
            "2".into(),
        ]
    );
    assert_eq!(spec.cwd, dir);
    assert_eq!(spec.env["PRESSURE_VESSEL_VERBOSE"], "1");
    assert_eq!(spec.env["SRUMAN_SESSION"], "exec:sniper:1");
    assert_eq!(spec.env["PRESSURE_VESSEL_FILESYSTEMS_RW"], dir.as_os_str());
    assert!(!spec.env.contains_key("PRESSURE_VESSEL_HOME"));
    assert!(!spec.env.contains_key("SRUMAN_PREFIX"));
    assert!(plan.prefix.is_none());
}

#[test]
fn bare_names_run_from_the_runtime_path_in_the_prefix_home() {
    let w = World::new();
    let plan = prepare(
        Target::Exe(Path::new("glxinfo")),
        &profile(),
        &Tag::Session("exec:indie:1".into()),
        &w.ctx(std::slice::from_ref(&w.bin), &[]),
    )
    .unwrap();
    let spec = plan.spec(&[]);
    assert_eq!(strings(spec.argv()).last().unwrap(), "glxinfo");
    assert_eq!(spec.cwd, w.home.join("prefixes/indie/home"));
    assert!(spec.shader_cache.is_none(), "shader caches are per game");
}

#[test]
fn every_problem_is_reported_at_once() {
    let w = World::new();
    let mut game = w.game();
    game.exe = "~/missing/game".into();
    game.cwd = Some("/nowhere".into());
    let mut profile = profile();
    profile.wrappers.push("nothere --x".into());
    profile.slr.as_mut().unwrap().runtime = "soldier".into();
    let err = prepare(
        Target::Game(&game),
        &profile,
        &Tag::Game("g".into()),
        &w.ctx(std::slice::from_ref(&w.bin), &[]),
    )
    .unwrap_err();
    let home = w.home.display();
    assert_eq!(
        err.problems,
        [
            format!("executable not found: {home}/missing/game"),
            "working directory not found: /nowhere".into(),
            "wrapper `nothere` not found in PATH".into(),
            "runtime `soldier` is not installed; run `sruman -S soldier`".into(),
        ]
    );

    let pinned = new_profile("p", &"sniper=3.0.1".parse().unwrap());
    let err = prepare(
        Target::Shell,
        &pinned,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert_eq!(
        err.problems,
        ["runtime `sniper=3.0.1` is not installed; run `sruman -S sniper=3.0.1`"]
    );
    let mut colon = new_profile("p", &"sniper".parse().unwrap());
    colon.slr.as_mut().unwrap().filesystems_rw = vec!["/a:b".into()];
    let err = prepare(
        Target::Shell,
        &colon,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert!(err.problems[0].contains("contains `:`"), "{err}");
}

#[test]
fn a_non_executable_or_broken_build_is_named() {
    let w = World::new();
    let file = w.home.join("game");
    fs::write(&file, "x").unwrap();
    let err = prepare(
        Target::Exe(&file),
        &new_profile("p", &"sniper".parse().unwrap()),
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert_eq!(
        err.problems,
        [format!(
            "{} is not executable; `chmod +x` it",
            file.display()
        )]
    );

    fs::remove_file(w.build_dir().join("run")).unwrap();
    let err = prepare(
        Target::Shell,
        &new_profile("p", &"sniper".parse().unwrap()),
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert!(
        err.problems[0].contains("is incomplete") && err.problems[0].contains("sruman -Qkk sniper"),
        "{err}"
    );
}

#[test]
fn acquire_holds_the_build_and_the_prefix_for_the_launch() {
    let _spawn = crate::test_lock::checks();
    let w = World::new();
    let game = w.game();
    let search = [w.bin.clone()];
    let plan = prepare(
        Target::Game(&game),
        &profile(),
        &Tag::Game("buckshot".into()),
        &w.ctx(&search, &[]),
    )
    .unwrap();
    let locks = w.tmp.path().join("run");
    let prefix_ctx = PrefixContext {
        lock_dir: &locks,
        home: &w.home,
    };
    let installed = w.store.resolve(&"sniper".parse().unwrap()).unwrap();
    let prefix_dir = w.home.join("prefixes/indie");
    assert!(!w.store.in_use(&installed));

    let launch = plan.acquire(&w.store, &prefix_ctx).unwrap();
    assert_eq!(launch.fds.len(), 2);
    assert!(w.store.in_use(&installed));
    assert!(prefix::in_use(&locks, &prefix_dir));
    assert!(prefix_dir.join("home/.config").is_dir());
    assert!(w.home.join(".cache/shaders/buckshot").is_dir());
    let argv = strings(launch.spec.argv());
    let passed: Vec<String> = launch
        .fds
        .iter()
        .map(|fd| format!("--pass-fd={}", fd.as_raw_fd()))
        .collect();
    let dashdash = argv.iter().position(|a| a == "--").unwrap();
    assert_eq!(argv[dashdash - 2..dashdash], passed[..]);
    // A removal can't take the build now.
    let lock = w.store.lock().unwrap();
    assert!(w.store.remove(&lock, &installed, false).is_err());
    drop(lock);

    drop(launch);
    assert!(!w.store.in_use(&installed));
    assert!(!prefix::in_use(&locks, &prefix_dir));
}

#[test]
fn acquire_fails_cleanly_on_a_busy_prefix() {
    let _spawn = crate::test_lock::checks();
    let w = World::new();
    let game = w.game();
    let plan = prepare(
        Target::Game(&game),
        &profile(),
        &Tag::Game("buckshot".into()),
        &w.ctx(std::slice::from_ref(&w.bin), &[]),
    )
    .unwrap();
    let locks = w.tmp.path().join("run");
    let prefix_dir = w.home.join("prefixes/indie");
    fs::create_dir_all(&prefix_dir).unwrap();
    let _reset = prefix::lock(&locks, &prefix_dir, true).unwrap();
    let err = plan
        .acquire(
            &w.store,
            &PrefixContext {
                lock_dir: &locks,
                home: &w.home,
            },
        )
        .unwrap_err();
    assert!(err.problems[0].contains("in use"), "{err}");
    let installed = w.store.resolve(&"sniper".parse().unwrap()).unwrap();
    assert!(!w.store.in_use(&installed), "the build hold was released");
}

#[test]
fn wrap_is_usable_on_its_own() {
    let w = World::new();
    let native = NativeCommand {
        program: "/opt/game/bin".into(),
        args: vec!["-a".into()],
        env: BTreeMap::from([("LOOTRS_GAME".into(), "g".into())]),
        cwd: "/opt/game".into(),
    };
    let profile = new_profile("lootrs-native", &"sniper".parse().unwrap());
    let plan = wrap(
        native,
        profile.slr.as_ref().unwrap(),
        &profile,
        None,
        &w.ctx(&[], &[]),
    )
    .unwrap();
    let spec = plan.spec(&[]);
    let build = w.build_dir().display().to_string();
    assert_eq!(
        strings(spec.argv()),
        [
            format!("{build}/run"),
            "--".into(),
            "/opt/game/bin".into(),
            "-a".into()
        ]
    );
    assert_eq!(spec.env["LOOTRS_GAME"], "g");
    assert_eq!(spec.env["PRESSURE_VESSEL_FILESYSTEMS_RW"], "/opt/game");
}

/// Runs a command in a real runtime: the prefix home receives the writes,
/// and the builds and prefix stay locked exactly while it runs. Either
/// `SRUMAN_REAL_BUILD=<extracted container build>` (linked as sniper), or
/// `SRUMAN_REAL_STORE=<store root> SRUMAN_REAL_RUNTIME=<name>`:
/// `cargo test -p libsruman --lib -- --ignored real_launch --nocapture`
#[test]
#[ignore = "needs a real runtime (SRUMAN_REAL_BUILD or SRUMAN_REAL_STORE)"]
fn real_launch() {
    use std::process::{Command, Stdio};

    // Before `spawns()`: `World::new` takes `checks()`.
    let mut w = World::new();
    let _spawn = crate::test_lock::spawns();
    let runtime = match std::env::var_os("SRUMAN_REAL_STORE") {
        Some(root) => {
            w.store = Store::open(root);
            std::env::var("SRUMAN_REAL_RUNTIME").unwrap_or_else(|_| "sniper".into())
        }
        None => {
            let real =
                PathBuf::from(std::env::var_os("SRUMAN_REAL_BUILD").expect("SRUMAN_REAL_BUILD"));
            let lock = w.store.lock().unwrap();
            let id: BuildId = "3.0.99".parse().unwrap();
            let meta = BuildMeta::new(
                "sniper",
                &id,
                Source::Link {
                    target: real.clone(),
                },
            );
            w.store
                .link(&lock, &real, meta, MakeCurrent::Always)
                .unwrap();
            "sniper".into()
        }
    };
    for mode in [SlrMode::Container, SlrMode::Overlay] {
        eprintln!("--- {mode}");
        let mut profile = new_profile("real", &runtime.parse().unwrap());
        profile.prefix = Some("~/prefixes/real".into());
        profile.slr.as_mut().unwrap().mode = mode;
        // The real host's libraries for overlay mode.
        w.host = HostLibs::system().unwrap();
        let extra: Vec<String> = vec![
            "-c".into(),
            "echo \"inside HOME=$HOME LD_LIBRARY_PATH=$LD_LIBRARY_PATH\"; echo saved > \"$HOME/marker\"; sleep 4".into(),
        ];
        let plan = prepare(
            Target::Exe(Path::new("/bin/sh")),
            &profile,
            &Tag::Session("test".into()),
            &w.ctx(&[], &extra),
        )
        .unwrap();
        let locks = w.tmp.path().join("run");
        let launch = plan
            .acquire(
                &w.store,
                &PrefixContext {
                    lock_dir: &locks,
                    home: &w.home,
                },
            )
            .unwrap();
        launch.make_inheritable().unwrap();
        let spec = &launch.spec;
        eprintln!("argv: {:?}", strings(spec.argv()));
        let child = Command::new(&spec.program)
            .args(&spec.args)
            .envs(&spec.env)
            .current_dir(&spec.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // sruman would exit here; only the child keeps the locks now.
        drop(launch);
        std::thread::sleep(std::time::Duration::from_secs(2));
        let prefix_dir = w.home.join("prefixes/real");
        for build in &plan.builds {
            assert!(
                w.store.in_use(build),
                "{} held by the running game",
                build.name
            );
        }
        assert!(
            prefix::in_use(&locks, &prefix_dir),
            "prefix held by the running game"
        );

        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("{stdout}");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("inside HOME="), "{stdout}");
        assert_eq!(
            fs::read_to_string(prefix_dir.join("home/marker")).unwrap(),
            "saved\n"
        );
        fs::remove_file(prefix_dir.join("home/marker")).unwrap();
        if let Some(overlay) = &plan.overlay {
            assert!(overlay.dir.is_dir(), "the overlay folder was built");
            assert!(stdout.contains(&*overlay.dir.to_string_lossy()), "{stdout}");
        }
        assert!(
            !w.home.join("marker").exists(),
            "the real home wasn't written"
        );
        for build in &plan.builds {
            assert!(!w.store.in_use(build), "{} released", build.name);
        }
        assert!(!prefix::in_use(&locks, &prefix_dir));
    }
}

impl World {
    /// A build that only has `marker` (scout's entry point).
    fn install_marker(&self, name: &str, build: &str, marker: &str) -> Installed {
        let lock = self.lock();
        let staged = self.store.staging_dir(&lock, name).unwrap();
        executable(&staged.join(marker));
        let id: BuildId = build.parse().unwrap();
        let meta = BuildMeta::new(name, &id, Source::Import { from: "/x".into() });
        self.store
            .commit(&lock, &staged, meta, MakeCurrent::IfNewer)
            .unwrap()
    }
}

#[test]
fn scout_runs_through_soldier() {
    let w = World::new();
    let soldier = w.install("soldier", "2.0.1");
    let scout = w.install_marker("scout", "1.0.1", "scout-on-soldier-entry-point-v2");
    let game = w.game();
    let mut profile = profile();
    profile.slr.as_mut().unwrap().runtime = "scout".into();
    profile.wrappers.clear();
    profile.shader_cache = None;
    profile.slr.as_mut().unwrap().filesystems_rw.clear();
    let plan = prepare(
        Target::Game(&game),
        &profile,
        &Tag::Game("buckshot".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap();
    let spec = plan.spec(&[5]);
    assert_eq!(
        strings(spec.argv()),
        [
            soldier.dir.join("run").display().to_string(),
            "--verbose".into(),
            "--pass-fd=5".into(),
            "--".into(),
            scout
                .dir
                .join("scout-on-soldier-entry-point-v2")
                .display()
                .to_string(),
            "--".into(),
            w.home
                .join("Games/Buckshot/Buckshot Roulette.x86_64")
                .display()
                .to_string(),
            "--fullscreen".into(),
        ]
    );
    assert_eq!(
        spec.env["PRESSURE_VESSEL_VARIABLE_DIR"],
        soldier.dir.join("var").as_os_str()
    );
    let rw = spec.env["PRESSURE_VESSEL_FILESYSTEMS_RW"]
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        rw,
        format!(
            "{}:{}",
            w.home.join("Games/Buckshot").display(),
            scout.dir.display()
        )
    );
    let names: Vec<&str> = plan.builds.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["scout", "soldier"], "both are held");
    assert_eq!(plan.prefix.as_ref().unwrap().runtime, "scout");
    assert_eq!(plan.prefix.as_ref().unwrap().home, HomeMode::Container);
}

#[test]
fn scout_without_soldier_names_the_fix() {
    let w = World::new();
    w.install_marker("scout", "1.0.1", "scout-on-soldier-entry-point-v2");
    let profile = new_profile("scout", &"scout".parse().unwrap());
    let err = prepare(
        Target::Shell,
        &profile,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert_eq!(
        err.problems,
        ["`scout` runs inside `soldier`, which is not installed; run `sruman -S soldier`"]
    );
}

const X64: &str = "x86_64-linux-gnu";

impl World {
    /// Gives the installed sniper build libraries: a newer SDL than the
    /// host's, an older zlib, a library the host lacks, and glibc 2.31.
    fn sniper_libs(&self) {
        fake_platform(
            &self.build_dir(),
            &format!("sniper_platform_{BUILD}"),
            &[
                (X64, "libSDL2-2.0.so.0", "libSDL2-2.0.so.0.3200.74", "sdl"),
                (X64, "libz.so.1", "libz.so.1.2.11", "z"),
                (X64, "libpng16.so.16", "libpng16.so.16.37.0", "png"),
                (X64, "libc.so.6", "libc.so.6", "GLIBC_2.2.5\0GLIBC_2.31\0"),
            ],
        );
    }
}

fn overlay_profile() -> Profile {
    let mut profile = profile();
    let slr = profile.slr.as_mut().unwrap();
    slr.mode = SlrMode::Overlay;
    slr.filesystems_rw.clear();
    slr.filesystems_ro.clear();
    profile.runner_args.clear();
    profile.shader_cache = None;
    profile
        .env
        .insert("LD_LIBRARY_PATH".into(), "/opt/profile/lib".into());
    profile
}

#[test]
fn overlay_runs_on_the_host_with_the_runtimes_newer_libraries_first() {
    let _spawn = crate::test_lock::checks();
    let w = World::new();
    w.sniper_libs();
    let game = w.game();
    let search = [w.bin.clone()];
    let plan = prepare(
        Target::Game(&game),
        &overlay_profile(),
        &Tag::Game("buckshot".into()),
        &w.ctx(&search, &[]),
    )
    .unwrap();
    let spec = plan.spec(&[3, 4]);
    let bin = w.bin.display().to_string();
    assert_eq!(
        strings(spec.argv()),
        [
            format!("{bin}/gamemoderun"),
            format!("{bin}/mangohud"),
            "--dlsym".into(),
            w.home
                .join("Games/Buckshot/Buckshot Roulette.x86_64")
                .display()
                .to_string(),
            "--fullscreen".into(),
        ],
        "the game itself: no entry point, no --pass-fd"
    );
    let overlay = plan.overlay.clone().unwrap();
    let build = w.store.resolve(&"sniper".parse().unwrap()).unwrap();
    assert_eq!(overlay.dir.parent().unwrap(), w.store.overlay_dir(&build));
    assert_eq!(
        spec.env["LD_LIBRARY_PATH"],
        OsString::from(format!(
            "{}:{}:/opt/profile/lib",
            overlay.dir.join(X64).display(),
            overlay.dir.join("i386-linux-gnu").display()
        ))
    );
    let linked: Vec<&str> = overlay.links[&Abi::X86_64]
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(linked, ["libSDL2-2.0.so.0", "libpng16.so.16"]);
    let home = w.home.join("prefixes/indie/home");
    assert_eq!(spec.env["HOME"], home.as_os_str());
    assert_eq!(
        spec.env["XDG_DATA_HOME"],
        home.join(".local/share").as_os_str()
    );
    assert!(
        spec.env.keys().all(|k| !k.starts_with("PRESSURE_VESSEL")),
        "{:?}",
        spec.env.keys()
    );
    assert_eq!(spec.env["SRUMAN_GAME"], "buckshot");
    assert_eq!(spec.env["A"], "game");
    let names: Vec<&str> = plan.builds.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["sniper"]);
    assert_eq!(plan.prefix.as_ref().unwrap().home, HomeMode::Env);
    assert!(!overlay.dir.exists(), "prepare doesn't write");

    // `acquire` builds the folder and holds the build.
    let locks = w.tmp.path().join("locks");
    let ctx = PrefixContext {
        lock_dir: &locks,
        home: &w.home,
    };
    let launch = plan.acquire(&w.store, &ctx).unwrap();
    assert_eq!(
        fs::read_link(overlay.dir.join(X64).join("libSDL2-2.0.so.0")).unwrap(),
        build.dir.join(format!(
            "sniper_platform_{BUILD}/files/lib/{X64}/libSDL2-2.0.so.0.3200.74"
        ))
    );
    assert!(w.store.in_use(&build));
    assert!(w.home.join("prefixes/indie/home/.config").is_dir());
    drop(launch);

    // Removing the build removes its overlays.
    let lock = w.lock();
    w.store.remove(&lock, &build, false).unwrap();
    assert!(!w.store.overlay_dir(&build).exists());
}

#[test]
fn overlay_without_a_prefix_keeps_the_real_home_and_shell() {
    let w = World::new();
    w.sniper_libs();
    let mut plain = new_profile("p", &"sniper".parse().unwrap());
    plain.slr.as_mut().unwrap().mode = SlrMode::Overlay;
    let plan = prepare(
        Target::Shell,
        &plain,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap();
    let spec = plan.spec(&[]);
    assert_eq!(strings(spec.argv()), ["/usr/bin/fish"]);
    assert!(!spec.env.contains_key("HOME"));
    assert!(!spec.env.contains_key("PRESSURE_VESSEL_SHELL"));
    assert_eq!(spec.cwd, w.home);
    assert!(plan.prefix.is_none());
    let path = spec.env["LD_LIBRARY_PATH"].to_string_lossy().into_owned();
    assert!(!path.ends_with(':'), "{path}");
}

#[test]
fn scout_in_overlay_mode_needs_no_soldier() {
    let w = World::new();
    let scout = w.install_marker("scout", "1.0.1", "scout-on-soldier-entry-point-v2");
    let lib = scout.dir.join(format!("steam-runtime/lib/{X64}"));
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("libpng12.so.0.46.0"), "png").unwrap();
    std::os::unix::fs::symlink("libpng12.so.0.46.0", lib.join("libpng12.so.0")).unwrap();
    let mut profile = new_profile("scout", &"scout".parse().unwrap());
    profile.slr.as_mut().unwrap().mode = SlrMode::Overlay;
    let game = w.game();
    let plan = prepare(
        Target::Game(&game),
        &profile,
        &Tag::Game("buckshot".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap();
    let names: Vec<&str> = plan.builds.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["scout"], "no soldier");
    let overlay = plan.overlay.as_ref().unwrap();
    assert_eq!(
        overlay.links[&Abi::X86_64]["libpng12.so.0"],
        lib.join("libpng12.so.0.46.0")
    );
    assert_eq!(
        strings(plan.spec(&[]).argv())[0],
        w.home
            .join("Games/Buckshot/Buckshot Roulette.x86_64")
            .display()
            .to_string()
    );
}

#[test]
fn overlay_refuses_a_runtime_newer_than_the_hosts_glibc() {
    let w = World::new();
    fake_platform(
        &w.build_dir(),
        &format!("sniper_platform_{BUILD}"),
        &[(X64, "libc.so.6", "libc.so.6", "GLIBC_2.2.5\0GLIBC_2.43\0")],
    );
    let mut profile = new_profile("p", &"sniper".parse().unwrap());
    profile.slr.as_mut().unwrap().mode = SlrMode::Overlay;
    let err = prepare(
        Target::Shell,
        &profile,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap_err();
    assert_eq!(
        err.problems,
        [
            "overlay mode needs the host's glibc to be at least the runtime's: the runtime has \
          2.43, the host 2.42; use `mode = \"container\"`"
        ]
    );
    // preflight says the same.
    let text = issue_text(&preflight(&profile, &w.ctx(&[], &[])));
    assert_eq!(text.len(), 1, "{text:?}");
    assert!(text[0].contains("2.43"), "{text:?}");
}

#[test]
fn preflight_names_overrides_that_cannot_apply() {
    let w = World::new();
    w.sniper_libs();
    let mut profile = new_profile("p", &"sniper".parse().unwrap());
    let slr = profile.slr.as_mut().unwrap();
    slr.mode = SlrMode::Overlay;
    slr.lib_overrides = BTreeMap::from([
        ("libSDL2-2.0.so.0".into(), LibOverride::Host),
        ("libz.so.1".into(), LibOverride::Runtime),
        ("libpng16.so.16".into(), LibOverride::Host),
        ("libnothere.so.1".into(), LibOverride::Runtime),
        ("libalso.so.2".into(), LibOverride::Newest),
    ]);
    let text = issue_text(&preflight(&profile, &w.ctx(&[], &[])));
    assert_eq!(
        text,
        [
            "warning: profile `p`: `slr.lib_overrides.\"libalso.so.2\"`: neither sniper \
             3.0.20261021.266025 nor the host has a x86_64-linux-gnu copy",
            "warning: profile `p`: `slr.lib_overrides.\"libnothere.so.1\"` = \"runtime\", but \
             sniper 3.0.20261021.266025 has no x86_64-linux-gnu copy",
            "warning: profile `p`: `slr.lib_overrides.\"libpng16.so.16\"` = \"host\", but the \
             host has no x86_64-linux-gnu copy",
        ]
    );
    // The overrides that apply change the folder.
    let plan = prepare(
        Target::Shell,
        &profile,
        &Tag::Session("s".into()),
        &w.ctx(&[], &[]),
    )
    .unwrap();
    let linked: Vec<&str> = plan.overlay.as_ref().unwrap().links[&Abi::X86_64]
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(linked, ["libz.so.1"]);
}

fn issue_text(issues: &[Issue]) -> Vec<String> {
    issues.iter().map(ToString::to_string).collect()
}

#[test]
fn preflight_is_empty_when_all_is_in_place() {
    let w = World::new();
    fs::create_dir_all(w.home.join("Music")).unwrap();
    fs::create_dir_all(w.home.join("Games/Buckshot")).unwrap();
    let mut profile = profile();
    profile.slr.as_mut().unwrap().filesystems_ro.clear();
    assert_eq!(
        issue_text(&preflight(
            &profile,
            &w.ctx(std::slice::from_ref(&w.bin), &[])
        )),
        Vec::<String>::new()
    );
}

#[test]
fn preflight_names_each_fix() {
    let w = World::new();
    w.install_marker("scout", "1.0.1", "scout-on-soldier-entry-point-v2");
    let mut profile = profile();
    profile.slr.as_mut().unwrap().runtime = "scout".into();
    profile.wrappers.push("nothere".into());
    let text = issue_text(&preflight(
        &profile,
        &w.ctx(std::slice::from_ref(&w.bin), &[]),
    ));
    let home = w.home.display();
    assert_eq!(
        text,
        [
            "error: profile `indie`: `scout` runs inside `soldier`, which is not installed; run `sruman -S soldier`".to_owned(),
            "error: profile `indie`: wrapper `nothere` not found in PATH".into(),
            format!("warning: profile `indie`: `slr.filesystems_rw`: {home}/Music doesn't exist, so it isn't shared"),
            format!("warning: profile `indie`: `slr.filesystems_rw`: {home}/Games/Buckshot doesn't exist, so it isn't shared"),
            "warning: profile `indie`: `slr.filesystems_ro`: /mnt/ro doesn't exist, so it isn't shared".into(),
        ]
    );

    // Validation errors come first and stop the rest.
    let mut broken = new_profile("broken", &"nope".parse().unwrap());
    broken.prefix = Some("relative".into());
    let text = issue_text(&preflight(&broken, &w.ctx(&[], &[])));
    assert_eq!(text.len(), 2, "{text:#?}");
    assert!(
        text.iter()
            .all(|t| t.starts_with("error: profile `broken`"))
    );

    // A prefix path that is a file.
    fs::write(w.home.join("afile"), "").unwrap();
    let mut file_prefix = new_profile("f", &"sniper".parse().unwrap());
    file_prefix.prefix = Some("~/afile".into());
    let text = issue_text(&preflight(&file_prefix, &w.ctx(&[], &[])));
    assert_eq!(
        text,
        [format!(
            "error: profile `f`: `prefix` {home}/afile is not a directory"
        )]
    );
}

#[test]
fn shells_per_runtime_kind() {
    let w = World::new();
    let soldier = w.install("soldier", "2.0.1");
    let scout = w.install_marker("scout", "1.0.1", "scout-on-soldier-entry-point-v2");
    let tag = Tag::Session("shell:p:1".into());
    for (runtime, with_prefix) in [("sniper", true), ("sniper", false), ("scout", true)] {
        let mut profile = new_profile("p", &runtime.parse().unwrap());
        profile.wrappers = vec!["gamemoderun".into()];
        if with_prefix {
            profile.prefix = Some("~/prefixes/p".into());
        }
        let plan = prepare(
            Target::Shell,
            &profile,
            &tag,
            &w.ctx(std::slice::from_ref(&w.bin), &[]),
        )
        .unwrap();
        let spec = plan.spec(&[]);
        let argv = strings(spec.argv());
        assert!(
            !argv.iter().any(|a| a.ends_with("gamemoderun")),
            "no wrappers for shells: {argv:?}"
        );
        assert_eq!(spec.env["PRESSURE_VESSEL_SHELL"], "instead", "{runtime}");
        assert_eq!(spec.env["PRESSURE_VESSEL_TERMINAL"], "tty");
        assert_eq!(spec.env["SRUMAN_SESSION"], "shell:p:1");
        let expected_cwd = if with_prefix {
            w.home.join("prefixes/p/home")
        } else {
            w.home.clone()
        };
        assert_eq!(spec.cwd, expected_cwd, "{runtime} {with_prefix}");
        assert_eq!(plan.prefix.is_some(), with_prefix);
        let first = if runtime == "scout" {
            soldier.dir.join("run")
        } else {
            w.build_dir().join("run")
        };
        assert_eq!(argv[0], first.display().to_string());
        if runtime == "scout" {
            assert!(
                argv.contains(
                    &scout
                        .dir
                        .join("scout-on-soldier-entry-point-v2")
                        .display()
                        .to_string()
                )
            );
        }
    }
    // The prefix home doesn't exist before `acquire`; that's not a problem.
    assert!(!w.home.join("prefixes/p").exists());
}
