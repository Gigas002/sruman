use std::{fs, os::unix::fs::PermissionsExt};

use super::*;
use crate::{
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
        let world = Self {
            store: Store::open(root.join("runtimes")),
            catalog: Catalog::builtin(),
            home,
            bin,
            tmp,
        };
        world.install("sniper", BUILD);
        world
    }

    fn install(&self, name: &str, build: &str) -> Installed {
        let lock = self.store.lock().unwrap();
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
    let mut profile = Profile::new("indie", "sniper".parse().unwrap());
    profile.prefix = Some("~/prefixes/indie".into());
    profile.wrappers = vec!["gamemoderun".into(), "mangohud --dlsym".into()];
    profile.runner_args = vec!["--verbose".into()];
    profile.env = BTreeMap::from([
        ("A".into(), "profile".into()),
        ("B".into(), "profile".into()),
    ]);
    profile.shader_cache = Some(ShaderCache::Dir("~/.cache/shaders".into()));
    profile.slr.filesystems_rw = vec!["~/Music".into(), "~/Games/Buckshot".into()];
    profile.slr.filesystems_ro = vec!["/mnt/ro".into()];
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
    let mut profile = Profile::new("sniper", "sniper".parse().unwrap());
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
    profile.slr.runtime = "soldier".parse().unwrap();
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

    let pinned = Profile::new("p", "sniper=3.0.1".parse().unwrap());
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
    let mut colon = Profile::new("p", "sniper".parse().unwrap());
    colon.slr.filesystems_rw = vec!["/a:b".into()];
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
        &Profile::new("p", "sniper".parse().unwrap()),
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
        &Profile::new("p", "sniper".parse().unwrap()),
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
    let profile = Profile::new("lootrs-native", "sniper".parse().unwrap());
    let plan = wrap(native, &profile.slr, &profile, None, &w.ctx(&[], &[])).unwrap();
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

/// Runs a command in a real container build: the prefix home receives the
/// writes, and the build and prefix stay locked exactly while it runs.
/// `SRUMAN_REAL_BUILD=~/games/sruman-test/runtimes/SteamLinuxRuntime_sniper
/// cargo test -p libsruman --lib -- --ignored real_container --nocapture`
#[test]
#[ignore = "needs an extracted container build in SRUMAN_REAL_BUILD"]
fn real_container_launch() {
    use std::process::{Command, Stdio};

    let real = PathBuf::from(std::env::var_os("SRUMAN_REAL_BUILD").expect("SRUMAN_REAL_BUILD"));
    let w = World::new();
    let lock = w.store.lock().unwrap();
    let id: BuildId = "3.0.99".parse().unwrap();
    let meta = BuildMeta::new(
        "sniper",
        &id,
        Source::Link {
            target: real.clone(),
        },
    );
    let installed = w
        .store
        .link(&lock, &real, meta, MakeCurrent::Always)
        .unwrap();
    drop(lock);

    let mut profile = Profile::new("real", "sniper=3.0.99".parse().unwrap());
    profile.prefix = Some("~/prefixes/real".into());
    let extra: Vec<String> = vec![
        "-c".into(),
        "echo \"inside HOME=$HOME\"; echo saved > \"$HOME/marker\"; sleep 4".into(),
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
    assert!(w.store.in_use(&installed), "build held by the running game");
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
    assert!(
        !w.home.join("marker").exists(),
        "the real home wasn't written"
    );
    assert!(!w.store.in_use(&installed));
    assert!(!prefix::in_use(&locks, &prefix_dir));
}
