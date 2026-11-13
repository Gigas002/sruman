# sruman — implementation plan

**sruman** (Steam Runtime Manager) is a pacman/paru-style package manager for
the **Steam Linux Runtime** (SLR) that works without Steam. It also runs native
Linux games inside those runtimes through lootrs-style games, profiles and prefixes:

```sh
wine hollow_knight.exe                          # Windows game in a wine prefix
sruman -p scout ./hollow_knight.x86_64          # native game in an SLR "prefix"
sruman hollow-knight                            # the same, as a named game from games.toml
sruman -S sniper                                # install a runtime
sruman                                          # = sruman -Syu: refresh + upgrade all
```

The workspace ships two crates, cloned from `rust-template`:

| Crate       | Kind | Binary   | Role                                                                          |
| ----------- | ---- | -------- | ----------------------------------------------------------------------------- |
| `libsruman` | lib  | —        | All logic: catalog, sync index, store, install/verify, profiles, prefixes, launch |
| `sruman`    | bin  | `sruman` | Thin paru-like CLI over `libsruman`: settings, logging, output, process control |

Both are **GPL-3.0-or-later**, the same as lootrs. `sruman` is CLI only: no
TUI and no GUI.

`lootrs` and `gaki` will later use `libsruman` as an optional feature (see
"Downstream integration"). That shapes the library API from the start, but the
work in those repos isn't part of this plan.

References: `docs/ARCHITECTURE.md` (layout, settings pipeline, quality gates);
`../lootrs` for profiles (`model::Profile`, named files, `validate` → `Vec<Issue>`),
prefixes (`prefix::ensure` reconciling against a state file, per-prefix flock,
exit 4 when busy), `launch::prepare`/`LaunchSpec`, XDG paths, process tags and
run logs; `../gaki` D9 for how a sibling tool shares `liblootrs`' model;
pacman/paru for the CLI grammar and output.

---

## Goals

1. **Package manager for SLR.** Refresh an index, search, show info, install
   (with dependencies), upgrade, downgrade/pin, verify, remove, and clean the
   cache, using pacman's verbs and output. Several builds of a runtime can be
   installed side by side.
2. **Games, profiles and prefixes like lootrs.** A `games.toml` names games
   (`exe`, `profile`, `args`, `cwd`, `env`), so `sruman <game>` and
   `sruman <path>` both work. Profiles are named files
   (`profiles/<name>.toml`, name = file stem). Each one names a runtime and an
   optional `prefix` path, and several profiles can share one prefix. The
   prefix holds the game's private `$HOME`, so saves and configs stay there,
   like a wine prefix's `drive_c/users`.
3. **Library first.** `libsruman` has no CLI parsing, no config-file discovery
   and no logging subscriber. Launching returns a value (`LaunchSpec`), so
   lootrs and gaki can splice it into their own commands.
4. **Slim builds.** Network code (HTTP, xz, sha256) sits behind a feature.
   Without it, `libsruman` still manages profiles and prefixes and launches
   runtimes that are already on disk.
5. **Never break a running game.** An upgrade installs next to the old build
   and switches over atomically. Removal and cache cleaning skip builds that are
   in use.

## Non-goals

- TUI or GUI. lootrs is the place for a games UI once it integrates `libsruman`.
- Running Windows games: that's wine, Proton and umu (lootrs covers them).
- Replacing Steam: no login, no Steamworks bridge. Games that need a running
  Steam client still go through Steam or gaki.
- Building runtimes from Debian packages (see D5).

---

## What Valve publishes (checked 2026-11-13 against `repo.steampowered.com`)

There are two trees per runtime generation: `steamrt<N>/apt/` and
`steamrt<N>/images/`.

**`images/`** holds the ready-to-run runtimes and is what sruman installs from:

| Suite      | Dir        | Archive(s) in a build dir                                                             | Latest stable (today)  |
| ---------- | ---------- | ------------------------------------------------------------------------------------- | ---------------------- |
| `scout`      | `steamrt1` | `SteamLinuxRuntime.tar.xz` (64 MB → 224 MB; Steam app 1070560)                    | `1.0.20261021.266054`  |
| `scout-ldlp` | `steamrt1` | `steam-runtime.tar.xz` (64 MB → 224 MB; plus `steam-runtime.tar.xz.checksum`)      | `1.0.20261021.266054`  |
| `soldier`    | `steamrt2` | `SteamLinuxRuntime_soldier.tar.xz` (168 MB → 666 MB; app 1391110)                  | `2.0.20261021.266042`  |
| `sniper`     | `steamrt3` | `SteamLinuxRuntime_sniper.tar.xz` (195 MB → 792 MB; app 1628350)                   | `3.0.20261021.266025`  |
| `steamrt4`   | `steamrt4` | `SteamLinuxRuntime_4.tar.xz` (175 MB → 694 MB; app 4183110), `…_4-arm64.tar.xz`     | `4.0.20261021.265997`  |

- **Channel pointers** are one-line files holding a build id:
  `images/latest-public-stable.txt`, `latest-public-beta.txt`,
  `latest-container-runtime-depot.txt` (what the Steam client ships), and for
  scout `latest-steam-client-general-availability.txt`. Some are empty for some
  suites.
- **Versioned build dirs** `images/<build_id>/` stay on the server (the
  `1.0.*`–`4.0.*` builds go back about a year: 18–28 per suite). Pinning and
  downgrading therefore work, like the Arch Linux Archive.
- **Each build dir** has `VERSION.txt`/`BUILD_ID.txt` (the build id),
  `SHA256SUMS` (covers every file), the archive(s), `*.os-release.txt`, and
  `*.manifest.dpkg`: a tab-separated list of every Debian package and version
  in the runtime. `images/update-timestamp.txt` changes whenever anything is
  published. The server supports HTTP `Range`.
- **No signatures** on images: integrity is HTTPS + `SHA256SUMS`.

**`apt/`** (`steamrt3/apt/dists/sniper{,_beta,-runtime-overlay}`, GPG-signed
`InRelease`) is a Debian 11–based archive of the individual packages the
images are built from: libraries, `pressure-vessel-relocatable`,
`steamrt-container`, `steam-runtime-tools`. It doesn't contain an assembled
runtime. See D5 for why sruman doesn't install from it, and how it still gets
apt-level version tracking.

### Spike results (2026-11-13, step 0.4)

All five archives were downloaded, checked against `SHA256SUMS`, extracted,
and used to run commands on this machine (CachyOS, unprivileged userns
enabled). Steam's own installs in `~/.local/share/Steam/steamapps/common`
(`SteamLinuxRuntime`, `_soldier`, `_4`) were read for comparison and have
the same layout.

- **Archive layout.** One top-level directory (`SteamLinuxRuntime_<x>/`, or
  `steam-runtime/` for scout-ldlp) holding `run`, `_v2-entry-point`,
  `VERSIONS.txt`, `mtree.txt.gz`, `toolmanifest.vdf`, `pressure-vessel/`
  (+ `pressure-vessel-arm64/`), and `<suite>_platform_<build>/` (`files/`,
  `metadata`, `usr-mtree.txt.gz`). Scout's archive holds
  `scout-on-soldier-entry-point-v2` plus a full `steam-runtime/`.
- **Entry types.** Only regular files, directories and *relative* symlinks
  (760 in scout, 6 in the containers): no absolute or escaping links, no
  hardlinks, no device nodes, no setuid. So the extractor can refuse anything
  else outright.
- **`-Qkk`.** Each archive ships `mtree.txt.gz` (`type`, `mode`, `size`,
  `sha256` per file), so verification uses Valve's own list.
- **Running.** `<build>/run -- <cmd>` works for soldier, sniper and steamrt4
  (≈0.5 s overhead warm). `run` is a small generated script that sets
  `PRESSURE_VESSEL_RUNTIME*`, `PRESSURE_VESSEL_COPY_RUNTIME=1`, and
  `--variable-dir=${PRESSURE_VESSEL_VARIABLE_DIR:-<build>/var}`.
  `_v2-entry-point` is only Steam's compat-tool interface, so sruman
  doesn't need it.
- **Scout.** `soldier/run -- <scout>/scout-on-soldier-entry-point-v2 -- <cmd>`
  works, using the `steam-runtime/` bundled inside the scout archive. So
  `scout` depends on `soldier` only, and `scout-ldlp` is independent.
  The scout entry point writes to `<scout build>/var/`.
  `steam-runtime/run.sh <cmd>` (scout-ldlp) runs on the host libraries with
  the scout `LD_LIBRARY_PATH`, with no container.
- **Private home.** `PRESSURE_VESSEL_HOME=<prefix>/home` (= `--home`, which
  implies `--unshare-home`) works outside Steam, for scout-on-soldier too.
  Inside, `$HOME` and `getpwent` both say `/home/<user>`, writes land in
  `<prefix>/home` (pressure-vessel creates `.cache`, `.config`,
  `.local/share` and `.steam`), and the real home is hidden except for the
  working directory and paths shared explicitly.
- **Sharing paths.** `PRESSURE_VESSEL_FILESYSTEMS_RO/RW` (`:`-separated) work
  (an RO path is visible and read-only). `STEAM_COMPAT_INSTALL_PATH`,
  `STEAM_COMPAT_MOUNTS` and `STEAM_COMPAT_LIBRARY_PATHS` are also honored.
  The cwd is always shared.
- **Install dir must stay writable.** With the build made read-only,
  pressure-vessel fails (`openat(<platform>/files/.ref): Permission denied`).
  The `var/` copy is 99.6% hardlinks to the build (7627/7660 files), so
  `var/` must stay on the same filesystem, i.e. inside the build dir as Steam
  has it.
- **In-use detection is free for container runtimes.** While a game runs,
  pressure-vessel holds a read **OFD** lock (`F_OFD_SETLK`, not `flock`)
  on `<var>/tmp-*/.ref`, the same protocol its own `--gc-runtimes` uses. A
  build is in use if a write-lock probe on any `var/tmp-*/.ref` fails, no
  matter which tool (Steam, umu, lootrs, sruman) started the game. The scout
  and scout-ldlp dirs get no such lock, so sruman holds its own there (see
  3.5).
- **Other pressure-vessel options useful later:** `--shell=instead` +
  `--terminal=tty` (`-X --shell`), `--terminate-timeout`
  (`-Xk`), `--pass-fd`, `--only-prepare` (pre-warm after install), `--test`
  (`-D`), and `bin/steam-runtime-system-info` (`-D`).
- **Hosts without 32-bit libraries** (this machine) only print a warning
  (`Cannot determine ld.so for i386-linux-gnu`). `-D` should explain it.
- **Real game (Buckshot Roulette, Godot 4.1, sniper), tested.** The Linux
  depot (2835573) was fetched with gaki's depot browser into
  `~/games/sruman-test/buckshot` (`Buckshot Roulette.x86_64` +
  `libsteam_api.so`; the Windows install stayed untouched). It ran with
  `PRESSURE_VESSEL_HOME=<prefix>/home`, cwd = the game dir, Steam **not**
  running:
  - The game ran normally: Vulkan on RADV, no DRM check, and no crash in
    25 s. SteamAPI only logs `S_API FAIL` and the game carries on. Steam
    games that call `SteamAPI_RestartAppIfNecessary` would still need Steam
    or gaki.
  - Saves and caches went to `<prefix>/home/.local/share/godot/app_userdata/…`,
    not the real home.
  - pressure-vessel still exposes the host Steam client
    (`~/.local/share/Steam/linux64/steamclient.so` was loaded) inside a
    private home, so a game *could* reach a running Steam. That's intended
    (Steam API), and USAGE.md should mention it.
  - SIGTERM to the `run` process stopped the whole tree with no leftovers,
    which is what `-Xk` needs.
  - The test setup stays in `~/games/sruman-test/` (game, sniper build,
    prefix, logs) for the Phase 3/4 integration tests.

---

## Decisions

All decisions below were confirmed on 2026-11-13, and the Phase 0 spike
verified the ones that depended on runtime behavior.

| #   | Question | Default |
| --- | -------- | ------- |
| D1  | **Where does the profile model live?** A: in `libsruman`, with lootrs depending on it. B: in `liblootrs`, with sruman depending on it. | **Settled: A, with lootrs' shape.** With both GPL, B no longer has a license cost, but it would still pull lootrs' wine/umu/proton model (`dxvk`, `dll_overrides`, `[umu]`…) into a tool that only runs native games, and every sruman release would wait on lootrs. So `libsruman::model::Profile` copies lootrs' generic fields by name and meaning (`name`, `prefix`, `runner_args`, `wrappers`, `debug`, `env`, `shader_cache`) and adds an `[slr]` section (`SlrSection`: runtime + container knobs). A sruman profile is then exactly a lootrs `native` profile plus `[slr]`. Later, lootrs only needs `slr: Option<libsruman::model::SlrSection>` on its `Profile` for one profile file to work in both tools. The dependency only goes downward: `liblootrs` → `libsruman`, never the reverse. |
| D2  | **Sync or async library?** lootrs is sync, gaki is tokio. | **Settled: sync.** Long operations take a `Progress` trait and a `CancelFlag`. HTTP goes through a `Fetch` trait, with a default `ureq` impl behind `download`, so gaki can plug in its own `reqwest` client (proxy and all) and call installs via `spawn_blocking`. |
| D3  | **License.** | **Settled: GPL-3.0-or-later** for both crates (fill `LICENSE`, workspace `license`, `deny.toml`). |
| D4  | **Versions and pin syntax.** | **Settled: Valve's build id** (`3.0.20261021.266025`) is the version, so pacman's `name=version` syntax works: `sruman -S sniper=3.0.20261021.266025`, or `runtime = "sniper=3.0.…"` in a profile. A bare `sniper` follows the newest installed build (per-suite `current` symlink). Build ids compare numerically, part by part. |
| D5  | **Install from apt or from images?** You suggested apt for getting runtimes and tracking updates. | **Settled: install from `images/`, and track updates the apt way on top of it.** The apt archive holds the hundreds of `.deb`s a runtime is built from, not the runtime itself. Installing from it would mean resolving Debian dependencies, unpacking debs, and assembling a pressure-vessel container the way Valve's image pipeline does: rebuilding the runtime instead of using the one games are tested against. `images/` already gives what apt would: channel pointers (an index), versioned build dirs (history and downgrades), `SHA256SUMS`, and per-build `manifest.dpkg` (exact package versions). sruman uses those manifests for apt-level tracking: `-Si`/`-Qi` list packages, `-Qu`/`-Su` show which packages change in an upgrade (e.g. `mesa 25.1 → 25.2`), and `-Ss libvulkan` searches package names across runtimes. The cost is that images are only checksum-verified over HTTPS, while apt is GPG-signed. A later, optional hardening step could cross-check `pressure-vessel-relocatable`/`steamrt-container` versions from a manifest against the signed apt `Packages` index. |
| D6  | **Channels.** | Three channel names map to pointer files: `stable` → `latest-public-stable` (**settled default**), `beta` → `latest-public-beta`, and `steam` → `latest-container-runtime-depot`, falling back to `latest-steam-client-general-availability` (what the Steam client ships). Channels can be set globally (`channel`) or per runtime (`[channels] sniper = "beta"`), like enabling `[testing]` in pacman. |
| D7  | **Scout.** | **Settled: two installable runtimes**, the way the server publishes them: `scout` (the scout-on-soldier container, `depends = ["soldier"]`, verified: the scout archive bundles its own `steam-runtime/`) and `scout-ldlp` (the plain LD_LIBRARY_PATH runtime: no container, no userns needed, less isolated). `-S scout` pulls the dependencies in, pacman-style. |
| D8  | **Catalog.** | **Settled:** a built-in table (suite name, server dir, archive name per arch, entry point, depends, channel fallbacks) that `config.toml` `[[runtime]]` entries can extend or override, so a mirror or a future `steamrt5` needs no release. |
| D9  | **CLI.** | **Settled: paru-like**, see "CLI" below. Bare `sruman` = `-Syu`. A bare target always **runs**: a game from `games.toml` (`sruman hollow-knight`), or a path (`sruman -p scout ./game`, `sruman ./game`), resolved as in D10. Searching is only `-Ss`, with no paru-style interactive bare search. The operations beyond pacman's are `-P` (profiles), `-X` (execute) and `-D` (doctor; unlike pacman's `-D` database operation). |
| D10 | **Games list?** | **Settled: yes, lootrs' `games.toml`.** `[[game]]` entries with lootrs' fields (`name`, `exe`, `profile`, `args`, `cwd`, `env`; no `game_id`, which is umu-only). A run target is resolved by a shell-like rule: if it contains `/` or starts with `~`, it's a **path**; otherwise it's a **game name** from `games.toml`. An unknown name that matches a file in the current directory fails with `error: no game 'foo' (did you mean ./foo?)`, so a name never silently runs a file. `-p` on a game overrides its profile for that run. |
| D11 | **Prefix home mechanism.** | **Settled (verified in the spike): pressure-vessel's private home.** A profile with a `prefix` runs with `PRESSURE_VESSEL_HOME=<prefix>/home`, so `$HOME` and `getpwent` both resolve to it, and the real home is hidden except the cwd and shared paths. sruman adds the exe's directory, `cwd`, the prefix, and (for scout) the scout build dir (RW, for its `var/`) to `PRESSURE_VESSEL_FILESYSTEMS_*`, plus the profile's `filesystems_rw/ro`. `scout-ldlp` has no container, so it falls back to env redirection (`HOME`, `XDG_*_HOME`), and `-Pk` warns that this is weaker there. |
| D12 | **Missing runtime at launch.** | **Settled: prompt.** On a TTY: `:: Runtime sniper is not installed. Install it now? [Y/n]`, then the normal `-S` transaction, then the launch. With `--noconfirm` or no TTY: exit 2 naming `sruman -S sniper`. `auto_install = true` installs without asking. |

---

## CLI (paru-like)

The grammar is pacman's: one **operation** (an uppercase flag), lowercase
**modifiers** scoped to that operation, then targets. Short flags combine
(`-Syu`, `-Scc`, `-Rdd`). A repeated modifier escalates, as in pacman.

### Runtimes (package management)

| Command                         | Does                                                                                                                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `sruman`                        | `-Syu`                                                                                                                               |
| `-S <rt>[=build]…`              | Install with dependencies. Transaction summary + `:: Proceed with installation? [Y/n]`. `--needed` skips installed ones; `-w` downloads only |
| `-Sy` / `-Syy`                  | Refresh the sync index (pointer `.txt`s, `VERSION.txt`, `SHA256SUMS`, manifests); `-yy` forces it even if `update-timestamp.txt` is unchanged |
| `-Su` / `-Syu [rt…]`            | Upgrade installed runtimes (all, or the named ones), with the package changelog from the manifest diff; `-uu` allows downgrading to the channel's build |
| `-Ss [regex]`                   | Search runtime names/descriptions; with `--pkg`, search the package manifests (`-Ss --pkg mesa`)                                      |
| `-Si <rt>[=build]`              | Remote info: build, channel, archive size, installed size, os-release, date, `depends`; `-ii` adds the full package list              |
| `-Sl [rt]`                      | List available builds (the versioned dirs) and mark the installed ones                                                                |
| `-Sc` / `-Scc`                  | Clean the download cache of archives for uninstalled builds / all archives; also removes installed builds beyond `keep_versions` that aren't pinned or in use |
| `-Q [rt]`                       | Installed runtimes as `name build` (`-Qq` gives names only)                                                                           |
| `-Qi <rt>`                      | Installed info, including **Required By**: the profiles that use it, and whether it's running now                                    |
| `-Qu`                           | Upgradable runtimes, `old -> new`                                                                                                     |
| `-Qk` / `-Qkk`                  | Verify an installed build: files present / content hashes (against the build's own `mtree.txt.gz`: type, mode, size, sha256) |
| `-Ql <rt>`                      | List files                                                                                                                            |
| `-R <rt>[=build]…`              | Remove. Refuses if a profile uses it or it's running; `-d` skips the profile check; `-dd` also removes builds that are in use (with a warning) |
| `-U <archive\|dir>`             | Install from a local archive or directory (Steam's `steamapps/common/SteamLinuxRuntime_sniper`, umu's `steamrt3`). `--link` registers a directory in place without copying it, and sruman never deletes linked builds |

### Profiles, prefixes, running

| Command                                 | Does                                                                                                         |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `sruman [-p <P>] <game\|path> [args…]` | Run (shorthand for `-X`). Every bare target runs, resolved as in D10. A game uses its own profile (`-p` overrides it); a path uses `-p` or `default_profile`. `args` follow the game's own `args` |
| `-X [-p P] <game\|path> [args…]`        | Run. Detaches by default; `--wait` streams output and returns the game's exit code; `--debug` turns on verbose runtime logging |
| `-X -p P --shell`                       | Interactive shell inside the profile's runtime and prefix                                                   |
| `-Xs` / `-Xk <game\|P\|pid>` / `-Xkk`   | Running games and sessions (status) / stop one (SIGTERM → timeout → SIGKILL) / stop all                      |
| `-G`                                    | List games (name, profile, runtime, running), lootrs' `list`                                                 |
| `-Gi <game>`                            | Show a game: resolved exe, cwd, profile, runtime build, prefix, merged env, latest run log                   |
| `-Gk [game]`                            | Check games (validation + preflight, including their profiles), lootrs' `check <game>`                      |
| `-Ge`                                   | Open `games.toml` in `$EDITOR`, then validate it                                                             |
| `-Gl <game>`                            | Print the latest run log (`--path` prints its path), lootrs' `logs`                                          |
| `-P`                                    | List profiles (name, runtime, prefix, in use)                                                                |
| `-Pi <P>`                               | Show a profile: resolved runtime build, prefix and its state, other profiles sharing the prefix              |
| `-Pk [P]`                               | Check profiles (validation + preflight: runtime installed, entry point, paths). This is lootrs' `check`      |
| `-Pn <P> -r <rt> [--prefix <dir>]`      | New profile file + prefix init. An existing `--prefix` is reused, not reset. This is lootrs' `init`         |
| `-Pe <P>`                               | Open the profile in `$EDITOR`, then validate it                                                              |
| `-Pd <P>` / `-Pdd <P>`                  | Delete the profile file / also delete its prefix, unless another profile shares it or it's in use           |
| `-D`                                    | Doctor: userns/bwrap, AppArmor userns restriction, the runtime's `steam-runtime-system-info`                 |

Global options: `--noconfirm`, `--color auto|always|never`, `-q`, `-v`/`-vv`,
`--config <file>`, and `--json` on `-Q`, `-Qi`, `-P`, `-Pi`, `-G`, `-Gi`, `-Xs`, `-Sl`
(for scripts and lootrs). Exit codes follow lootrs: `0` ok, `1` runtime
failure, `2` invalid input or config, `3` not running, `4` busy (game
already running, prefix or build in use).

Output follows pacman: `::` section headers, `resolving dependencies...`,
`Runtimes (2)  soldier-2.0.20261021.266042  scout-1.0.20261021.266054`,
`Total Download Size`/`Total Installed Size`, one progress bar per download,
and `warning:`/`error:` prefixes. Colors only when stdout is a TTY, unless
`--color` says otherwise.

**Parsing:** clap derive, with each operation a `bool` short flag in one
exclusive `ArgGroup` and modifiers as `ArgAction::Count`. The `settings/`
resolver turns the flag soup into a typed `Operation` enum (`Sync { refresh: u8, upgrade: u8, search, info: u8, … }`)
and rejects modifiers that don't belong to the chosen operation, with a
pacman-like message (`error: invalid option '-k' for -S`). Arguments after the
executable pass through untouched (`trailing_var_arg`). Nothing below
`settings/` sees clap types.

---

## Profiles and prefixes (lootrs model)

`~/.config/sruman/profiles/scout.toml`:

```toml
name = "scout"                                   # must equal the file stem
prefix = "~/.local/share/sruman/prefixes/indie"  # optional; shared by any profile that names it
# runner_args = []                               # passed to the runtime entry point
# wrappers = ["gamemoderun", "mangohud"]         # outside the container, like Steam's %command%
# debug = false                                  # PRESSURE_VESSEL_VERBOSE + runtime logs
# shader_cache = "~/.cache/sruman/shaders"       # per-game dirs, lootrs' variable set
# [env]
# SDL_VIDEODRIVER = "wayland"

[slr]                                            # libsruman::model::SlrSection
runtime = "scout"                                # or "scout=1.0.20261021.266054" to pin
# filesystems_rw = ["~/Games"]
# filesystems_ro = []
```

- **Named files** like lootrs: the file stem is the name, names are validated
  with lootrs' rules (`[A-Za-z0-9._-]+`), validation collects every `Issue`
  instead of failing fast, and unknown or misplaced fields warn instead of
  failing.
- **Implicit profiles:** every catalog runtime name (`sniper`, `scout`, …)
  works as a profile with defaults and no prefix, so `sruman -p sniper ./game`
  works without any file. A real file with the same name overrides it.
  `default_profile` in `config.toml` is used when `-p` is omitted.
- **Prefixes** are plain directories referenced by path, so any number of
  profiles can share one (a scout profile and a sniper profile both using
  `prefixes/indie`). `prefix::ensure` runs on every launch, as in lootrs:
  it creates `home/` and the XDG skeleton, then reconciles `<prefix>/.sruman.toml`
  (sruman version, home mode, runtimes last used). Locking is a shared flock
  while a game runs and an exclusive one for `-Pdd` or a prefix reset, so a
  busy prefix gives exit 4. The lock files live in `$XDG_RUNTIME_DIR/sruman/`,
  keyed by a hash of the prefix path (lootrs' scheme).
- **No prefix** means the game sees the real home, the same as Steam's default.

## Games (lootrs model)

`~/.config/sruman/games.toml`, in lootrs' format and with the same
validation rules:

```toml
[[game]]
name = "hollow-knight"                     # unique id: `sruman hollow-knight`
exe = "~/Games/HollowKnight/hollow_knight.x86_64"
profile = "scout"                          # optional; default: default_profile
# args = ["-force-vulkan"]
# cwd = "~/Games/HollowKnight"             # default: the exe's directory
# env = { SDL_VIDEODRIVER = "wayland" }    # merged over the profile's env

[[game]]
name = "supertux"
exe = "~/Games/SuperTux/run.sh"
profile = "sniper"                         # implicit profiles work here too
```

- **Names** use lootrs' rules and must be unique. `exe` and `cwd` must be
  absolute after `~` expansion. `profile` must name a profile file or an
  implicit profile. Validation collects every issue (`-Gk`).
- **Env layering** follows lootrs: runtime env < profile `env` < game `env`.
  Wrappers come from the profile.
- **One instance per game**, as in lootrs: running a game that's already
  running exits 4. The process tag `SRUMAN_GAME=<name>` drives `-Xs`/`-Xk`.
- **Per-game state:** the shader cache goes in `<shader_cache>/<game>`, and
  run logs go in `logs/games/<game>/` with a `latest.log` link. Path runs
  log under `logs/exec/<profile>/`.
- The file shape matches lootrs' `games.toml` minus `game_id`, so lootrs
  games and sruman games read the same way (and a later lootrs integration
  can point a game at an `[slr]` profile without new fields).

---

## Target layout

```text
sruman/
  Cargo.toml  deny.toml  LICENSE (GPL-3.0)  CHANGELOG.md  README.md
  examples/
    config.toml
    games.toml
    profiles/{sniper.toml,scout.toml,shared-prefix-a.toml,shared-prefix-b.toml}
  clippy.toml                # disallowed-methods (5.5a)
  .github/workflows/audit.yml  # fuzz, mutants, benches, canary (5.5)
  fuzz/                      # cargo-fuzz targets + corpora (5.5a)
  docs/
    ARCHITECTURE.md  PLAN.md  USAGE.md  RUNTIMES.md  AUDIT.md
  libsruman/
    src/
      lib.rs
      paths/     # XdgDirs/AppDirs, expand_tilde (shape of liblootrs::paths)
      catalog/   # RuntimeDef, built-ins, config overrides, dependency resolution
      version/   # BuildId, RuntimeRef ("sniper" | "sniper=<id>"), Channel
      manifest/  # manifest.dpkg parse, diff (package changelog), search
      sync/      # local sync index: refresh, query, staleness via update-timestamp  [download]
      fetch/     # Fetch trait; UreqFetch                                           [download]
      install/   # transaction: resolve → download (cache, Range resume) → verify → extract → commit [download]
      store/     # installed builds, `current`, remove, clean, verify, locks, -U import/link
      model/     # Game, Profile, SlrSection, validate → Vec<Issue>, implicit profiles, target resolution
      prefix/    # ensure/reset/state file/locks (lootrs' prefix pattern)
      launch/    # prepare() → LaunchSpec, preflight()
      process/   # session tags, list/stop (no spawning)
      diag/      # userns checks, steam-runtime-system-info                         [diag]
      (each: mod.rs + tests.rs)
    tests/       # fixture HTTP server, temp store, examples load
  sruman/
    src/
      main.rs
      cli/ config/ settings/ logger/ utils/
      output/    # pacman-style printer: colors, ::, tables, prompts, progress bars, --json
      app/
        commands/    # one handler per operation: sync, query, remove, upgrade, profile, games, exec, doctor
        supervisor/  # spawn LaunchSpec, setsid, detach/--wait, run logs
    tests/cli.rs
```

## Storage

```text
$XDG_CONFIG_HOME/sruman/config.toml, games.toml, profiles/<name>.toml
$XDG_DATA_HOME/sruman/runtimes/<suite>/<build_id>/   # never modified by sruman after install; must stay user-writable (pressure-vessel opens files/.ref)
                                     /<build_id>/var/      # pressure-vessel variable dir (hardlinked copies: same fs required)
                                     /<build_id>/.sruman.toml  # source URL, archive sha256, linked?, installed-at
                                     /<build_id>/.sruman.lock  # sruman's own in-use lock (scout, scout-ldlp)
                                     current -> <build_id>
$XDG_DATA_HOME/sruman/prefixes/<name>/               # default prefix_root (prefix paths may be anywhere)
$XDG_CACHE_HOME/sruman/sync/<suite>.toml             # -Sy index: channel → build, sizes, sums, manifest
$XDG_CACHE_HOME/sruman/pkg/<suite>-<build>.tar.xz    # download cache (pacman's /var/cache/pacman/pkg)
$XDG_STATE_HOME/sruman/logs/                         # app log; games/<game>/, exec/<profile>/ run logs (lootrs runlog format)
$XDG_RUNTIME_DIR/sruman/                             # locks: per build in use, per prefix
```

`config.toml` (every key optional):

```toml
log_level = "info"
default_profile = "sniper"
channel = "stable"            # stable | beta | steam
keep_versions = 2             # installed builds kept per runtime by -Sc (pinned/in-use always kept)
auto_install = false          # missing runtime at launch: false → [Y/n] prompt on a TTY, exit 2 otherwise (D12)
# color = "auto"
# prefix_root = "~/.local/share/sruman/prefixes"
# [channels]
# sniper = "beta"
# [[runtime]]                 # extend/override the catalog (D8)
# name = "sniper"
# base_url = "https://mirror.example/steamrt3/images"
```

## Feature matrix

| Crate       | Feature    | Default | Pulls in                                   | Gives                                    |
| ----------- | ---------- | ------- | ------------------------------------------ | ---------------------------------------- |
| `libsruman` | `download` | yes     | `ureq` (rustls), xz decoder, `tar`, `sha2` | sync index, install, upgrade             |
| `libsruman` | `diag`     | no      | —                                          | doctor checks                            |
| `sruman`    | `download` | yes     | `libsruman/download`                       | `-S`, `-Sy`, `-Su`, `-Ss`, `-Si`, `-Sl`  |
| `sruman`    | `json`     | yes     | `serde_json`                               | `--json`                                 |

`serde` is not optional in `libsruman`: profiles, state files and the index
all need it. `--no-default-features` still gives `-Q`, `-R`, `-U`, `-P`, `-X`
on runtimes Steam or umu installed. Choose the xz crate in step 2.4 under
ARCHITECTURE §7.2, preferring pure Rust (check whether gaki's `lzma-rust2`
reads `.xz`) over `xz2`/C liblzma.

---

## Library API sketch (what lootrs and gaki call)

```rust
let dirs = AppDirs::new(&XdgDirs::from_lookup(std::env::var_os)?);
let catalog = Catalog::builtin().with_overrides(user_defs);

// Store (no network)
let store = Store::open(&dirs.runtimes)?;
store.installed() -> Vec<Installed>;
store.resolve(&"sniper=3.0.20261021.266025".parse::<RuntimeRef>()?) -> Result<Installed, ResolveError>;
store.remove(&RuntimeRef, RemoveMode) / store.clean(&CleanPolicy) / store.verify(&Installed, Depth);
store.import(path, ImportMode::{Copy, Link});

// Sync + transactions (feature "download")
let index = SyncIndex::open(&dirs.sync_cache);
index.refresh(&catalog, &fetch, Force::No) -> RefreshReport;           // -Sy
let tx = Transaction::install(&index, &store, &catalog, &[refs], &opts)?; // resolves depends, -S
tx.summary() -> TxSummary { items, download_size, installed_size, package_changes };
tx.commit(&fetch, &mut impl Progress, &CancelFlag) -> Vec<Outcome>;
Transaction::upgrade(&index, &store, &catalog, Upgrade::All, &opts)?;    // -Su

// Profiles and prefixes (lootrs pattern)
let profiles = model::load_dir(&dirs.profiles) -> (Vec<Profile>, Vec<Issue>);
let games = model::load_games(&dirs.games_file) -> (Vec<Game>, Vec<Issue>);
model::resolve_target("hollow-knight", &games, cwd) -> Result<RunTarget::{Game(&Game), Path(PathBuf)}, TargetError>; // D10 rule
profile.validate(&ValidationContext { home, catalog: &catalog }) -> Vec<Issue>;
prefix::ensure(&prefix_path, &profile.slr, &PrefixContext { lock_dir }) -> Result<PrefixGuard, PrefixError>;

// Launch: pure apart from fs lookups; spawning stays in the caller
launch::prepare(Target::{Game(&Game), Exe{path,args}, Shell}, &profile, &installed, &LaunchContext { home, search_path, debug, extra_args, artifacts_dir })
    -> Result<LaunchSpec, LaunchError /* collects all problems */>;
// LaunchSpec { program, args, env, cwd, keep_fds: Vec<OwnedFd> /* scout locks only */, prefix_guard: Option<PrefixGuard> }
launch::wrap(native: NativeCommand, &SlrSection, &installed, &ctx) -> Result<LaunchSpec, LaunchError>; // for lootrs/gaki
```

Rules:

- No `clap`, no subscriber, no config discovery (ARCHITECTURE §9). `AppDirs`
  is public so all three tools agree on the shared store location.
- `wrap` is the integration point. It takes a finished native command (what
  lootrs' native runner or gaki would spawn, with wrappers already outside)
  and returns it wrapped in the runtime entry point plus prefix env.
- **In use** = pressure-vessel's OFD read lock on any `<build>/var/tmp-*/.ref`
  (container runtimes, any launcher), or sruman's own lock on
  `<build>/.sruman.lock` (scout, scout-ldlp). The scout lock fd goes to the
  child without CLOEXEC (through the container with `--pass-fd`), so it
  outlives a detached `sruman`. `-R`/`-Sc` from any tool respect both.
- Errors are `thiserror` enums per module. Validation and launch collect every
  problem, as lootrs does. Read-mostly public structs are `#[non_exhaustive]`
  and have constructors, so new knobs aren't breaking changes.

---

## Steps

### Phase 0 — housekeeping and spike

- [x] **0.1** Rename crates: `crate/` → `libsruman/`, `binary-crate/` →
      `sruman/` (`[[bin]] name = "sruman"`). Fix workspace members, the
      `library-crate` alias, and drop the template `extra` features.
      _Done 2026-11-13:_ both crates renamed; the template's greet code and
      `extra` features are gone, so `libsruman` exports only `VERSION` and
      `sruman` prints it. Build, tests, clippy and fmt pass.
- [x] **0.2** Metadata: GPL-3.0-or-later in `[workspace.package]`, full GPL
      text in `LICENSE`, the `deny.toml` license allowlist, description,
      keywords, repository, and the template `TODO`s in `test.yml`/`deploy.yml`.
      _Done 2026-11-13:_ GPL-3.0-or-later in `[workspace.package]` with the
      GPL v3 text (same as lootrs), description, keywords, homepage and
      repository; `deny.toml` takes lootrs' license allowlist; `deploy.yml`
      publishes `libsruman` then `sruman`; codecov covers both crates.
      `.typos.toml` allows `Pn` (the `-Pn` flag). `cargo deny check` and
      `typos` pass.
- [x] **0.3** Short README (what, install, quick start, links). Start
      `CHANGELOG.md`.
      _Done 2026-11-13:_ README covers what sruman is, its status (early,
      nothing works yet), planned usage, requirements, file locations and
      building. The CHANGELOG has an `Unreleased` entry.
- [x] **0.4** **Spike.** _Done 2026-11-13:_ see "Spike results" above.
      Copy them into `docs/RUNTIMES.md` in 5.1.

### Phase 1 — library core (no network)

- [x] **1.1** `paths`: `XdgDirs`, `AppDirs` (config, profiles, runtimes,
      prefix root, sync cache, pkg cache, logs, locks), `expand_tilde`. Same
      shape as `liblootrs::paths`.
      _Done 2026-11-13:_ XdgDirs (now with cache_home) and AppDirs (config,
      profiles, games_file, runtimes, prefixes, sync_cache, pkg_cache, logs,
      locks with the state-dir fallback) and expand_tilde, in
      liblootrs::paths' shape; 7 tests. thiserror is the first dependency.
- [x] **1.2** `version`: `BuildId` (parse, numeric `Ord`, `Display`),
      `RuntimeRef` (`name` | `name=build`), `Channel` + pointer-file fallbacks.
      _Done 2026-11-13:_ BuildId (parsed from Valve's dot-separated ids,
      including the old 0.2022… style; ordered numerically part by part, with
      ties broken by spelling so Ord agrees with Eq), RuntimeRef (name or
      name=build, runtime names follow the profile-name rules), and Channel
      (stable/beta/steam, with the pointer-file fallback list per channel).
      All three serialize as their strings. serde becomes a dependency; toml
      is a dev-dependency for now. 8 tests.
- [ ] **1.3** `catalog`: built-in `RuntimeDef`s from the table above
      (`server_dir`, `archives[arch]`, `entry`, `depends`, `channels`),
      `with_overrides`, and dependency resolution (topological, with cycle
      and missing-dependency errors).
- [ ] **1.4** `manifest`: parse `manifest.dpkg`, diff two manifests
      (added/removed/changed packages with versions), and search by package
      name. Golden tests against a real manifest fixture.
- [ ] **1.5** `store`: layout, `installed`, `resolve`, atomic `set_current`
      (temp symlink + rename), `remove`, `clean(keep, pinned, in_use)`,
      `verify` (against the build's `mtree.txt.gz`), `import` (copy or
      link; detect the suite and build from `VERSION.txt`/`BUILD_ID.txt` and
      the entry point), and in-use detection (an OFD write-lock probe on
      `var/tmp-*/.ref`, plus `.sruman.lock`; rustix `fcntl_lock` /
      `flock`). Tests: "clean skips locked and pinned builds" (with a test
      holding an OFD read lock the way pressure-vessel does), "remove of a linked build only unregisters it",
      "resolve of a pinned build that's missing names the `sruman -S
      name=build` fix".
- [ ] **1.6** `model`: `Profile` (lootrs field names), `SlrSection`,
      `validate_name` (lootrs rules), `validate` → `Vec<Issue>`, `load_dir`,
      implicit profiles from the catalog. Fixtures under
      `libsruman/tests/fixtures/profiles/`, plus a test that every file in
      `examples/profiles/` loads and validates.
- [ ] **1.6b** `model::Game` + `load_games` (lootrs' `games.toml` fields
      minus `game_id`; unique names, absolute `exe`/`cwd`, the profile
      exists), and `resolve_target` (the D10 rule: `/` or `~` → path,
      otherwise a game name; an unknown name that matches a file in the cwd
      errors with the `./name` hint). Tests cover every branch of the rule,
      and `examples/games.toml` loads and validates.
- [ ] **1.7** `prefix`: `ensure` (skeleton, state file, reconcile), `reset`,
      shared/exclusive locks keyed by path hash, `Busy` error, and "which
      profiles share this prefix".

### Phase 2 — sync and transactions (`download`)

- [ ] **2.1** `fetch`: the `Fetch` trait (`get`, `get_range`, `head`), and
      `UreqFetch` (rustls, generic User-Agent `sruman/<ver>`, no user data).
      Fixture-based tests, no network.
- [ ] **2.2** `sync`: `refresh` reads `images/update-timestamp.txt` and skips
      the rest when it's unchanged (unless forced). Otherwise, per catalog
      runtime and channel, it reads the pointer `.txt`, then the build's
      `SHA256SUMS`, sizes (`HEAD`), `os-release`, and `manifest.dpkg`, and
      writes `sync/<suite>.toml`. `list_builds` (for `-Sl`) parses the
      nginx autoindex of `images/` for `N.0.*` dirs, keeping the parser
      tolerant and tested on a saved page.
- [ ] **2.3** `Progress` trait (`begin(item, total)`, `advance(n)`,
      `phase(Download|Verify|Extract|Commit)`) and `CancelFlag`.
- [ ] **2.4** `install::Transaction`: resolve targets + `depends` (pacman
      order), skip installed with `needed`, then a summary with sizes and
      manifest diffs. `commit` streams each archive into `pkg/` with `Range`
      resume while hashing, verifies against `SHA256SUMS`, extracts into
      `.staging/` (accepting only regular files, directories and relative
      symlinks that stay inside the root, the only kinds Valve's archives
      contain, and refusing everything else), strips the single top-level
      directory, creates `var/`, renames into place, and flips `current`. Each item
      commits atomically and a failure leaves earlier items installed (pacman
      semantics). Pick and justify the xz and tar crates here.
- [ ] **2.5** Upgrade (`Upgrade::All | Named`), with `-uu` allowing a build
      older than `current` when the channel points there. Run `clean` after
      if configured.
- [ ] **2.6** Integration tests: a small `std::net` HTTP fixture (no new
      dependency) serving a fake `images/` tree. Cover refresh, install with
      dependencies, upgrade, pin, downgrade, resume after a truncated
      download, and a checksum mismatch. Add one `#[ignore]`d test against the
      real server, with manual steps in `docs/RUNTIMES.md`.

### Phase 3 — launch

- [ ] **3.1** `launch::prepare`/`wrap` for container runtimes:
      `<build>/run [runner_args] -- <exe> <args>` (not `_v2-entry-point`), with
      `PRESSURE_VESSEL_VARIABLE_DIR=<build>/var`,
      `PRESSURE_VESSEL_FILESYSTEMS_RW/RO` (`:`-joined, `~` expanded: the
      prefix, exe dir, cwd and profile paths), `PRESSURE_VESSEL_HOME` (D11), `PRESSURE_VESSEL_VERBOSE` on
      `debug`, shader cache variables (lootrs' Mesa, NVIDIA and AMD set), and
      tags `SRUMAN_GAME` (game runs) or `SRUMAN_SESSION` (path runs and shells), plus
      `SRUMAN_PROFILE`/`SRUMAN_PREFIX`. For `Target::Game`: the game's `args`
      then the extra args, `cwd` defaulting to the exe's directory, env
      layered runtime < profile < game, and the shader cache in `<dir>/<game>`. Wrappers go in
      front of everything, as in lootrs.
- [ ] **3.2** `scout`: `<soldier>/run -- <scout>/scout-on-soldier-entry-point-v2 -- <exe>`,
      with the scout build dir shared RW. `scout-ldlp`: `<build>/run.sh <exe>`,
      prefix home through env only. Both hold `.sruman.lock` (`--pass-fd` for
      scout).
- [ ] **3.3** `preflight(profile)` → problems without launching, each
      naming its fix (`sruman -S scout`, `sruman -Pn …`).
- [ ] **3.4** `Target::Shell` for `-X --shell` (`PRESSURE_VESSEL_SHELL=instead`,
      `PRESSURE_VESSEL_TERMINAL=tty`; scout-ldlp: `run.sh $SHELL`). Golden argv/env tests per
      runtime kind and per target (game, path, shell), with and without a
      prefix.
- [ ] **3.5** `process`: list sessions by tag from `/proc`, and stop a
      process group (TERM, then a timeout, then KILL). lootrs' approach, not
      its code.
- [ ] **3.6** `diag` (feature): `unprivileged_userns_clone`,
      `max_user_namespaces`, Ubuntu's AppArmor userns restriction, and a
      wrapper around the runtime's `steam-runtime-system-info`.

### Phase 4 — the `sruman` binary

- [ ] **4.1** `cli/`: the pacman grammar above (operations group, counted
      modifiers, `-p`, trailing args). `settings/`: flags + `config.toml` +
      defaults → `Settings { operation: Operation, … }`, with pacman-style
      errors for invalid modifier combinations. Table-driven parse tests for
      every row of the CLI tables.
- [ ] **4.2** `logger/`: tracing to stderr (`-v`) and a daily file, as in
      lootrs.
- [ ] **4.3** `output/`: the pacman look (`::`, `resolving dependencies...`,
      transaction table, sizes, `[Y/n]` prompts honoring
      `--noconfirm`, multi-bar download progress on stderr, colors with TTY
      detection) and `--json` serializers. Hand-rolled unless a maintained,
      light crate clearly pays for itself (§7.2).
- [ ] **4.4** `app/commands`: one module per operation (sync, query,
      remove, upgrade-local, profile, exec, doctor). Thin over `libsruman`.
      `exec` handles D12: an install prompt, then the launch.
- [ ] **4.5** `app/supervisor`: spawn the `LaunchSpec` with `setsid`,
      detached by default or `--wait`. Per-run logs in lootrs' runlog format
      (command, env set, runtime build + os-release, system header, output,
      exit status) with `log_keep` rotation, in `logs/games/<game>/` (with a
      `latest.log` link) or `logs/exec/<profile>/`. Refuse a second instance
      of a running game (exit 4).
- [ ] **4.6** `tests/cli.rs`: parse matrix, `-Pk`/`-Gk` on examples, and
      `sruman <game>` / `sruman <path>` against a fake runtime whose `run` is a script echoing argv/env, run
      against the fixture HTTP server from 2.6 for `-S`/`-Syu`.

### Phase 5 — docs, audit and release

- [ ] **5.1** `docs/USAGE.md` (operations reference in pacman-manual style,
      games, profiles, shared prefixes, game-vs-path resolution, wrappers,
      logs, exit codes) and
      `docs/RUNTIMES.md` (suites, channels, server layout, apt vs images,
      import from Steam/umu, userns troubleshooting, manual test list).
- [ ] **5.2** Annotated `examples/config.toml`, `examples/games.toml` and
      `examples/profiles/*.toml`,
      loaded by tests.
- [ ] **5.3** Run the quality gates (§8) at all three feature levels, with
      `#![warn(missing_docs)]` on `libsruman`.
- [ ] **5.4** Manual checks: one game per runtime (Hollow Knight on scout, a
      sniper-era title); two profiles sharing a prefix; upgrade while a game
      runs (the old build stays, `-Sc` skips it); `-U --link` of Steam's
      sniper.
- [ ] **5.5** **Automated audit.** Tooling and tests that run on every audit
      pass, not a one-off read-through. Wire it into
      `.github/workflows/audit.yml` (weekly schedule, on tags,
      `workflow_dispatch`), and list each job's local command in
      `docs/AUDIT.md`. Results go into `docs/AUDIT.md` in gaki's format.
      Severity is **high** (code execution, or writes outside sruman's dirs
      and the user's prefixes), **medium** (crash, hang or unbounded
      allocation from outside input, corrupted store), or **low** (hardening).
      Each finding is **fixed**, **accepted** with a reason, or turned into a
      **follow-up** step 5.6x. New dev-dependencies (`proptest`, `insta`,
      `criterion`) are justified under §7.2 and are dev-only.
  - [ ] **5.5a Security.**
    - _Static:_ `#![forbid(unsafe_code)]` in both crates (rustix covers
      flock/setsid/kill safely). `clippy.toml` `disallowed-methods` bans
      `std::process::Command::new` outside `app/supervisor` and
      `diag`, and bans any `sh -c`. Deny `clippy::unwrap_used`,
      `expect_used` and `indexing_slicing` in modules that parse outside
      data (`fetch`, `sync`, `manifest`, `install`, `store::import`).
      `cargo deny check` covers advisories, sources and bans.
    - _Fuzzing_ (`fuzz/`, `cargo-fuzz`, nightly, 5 min per target in CI):
      `SHA256SUMS`, `VERSION.txt`/pointer files, `manifest.dpkg`, the
      autoindex parser, `BuildId`/`RuntimeRef`, profile/config/state TOML,
      and the tar extractor fed arbitrary archives. Commit the corpora, and
      add every crash as a regression test.
    - _Hostile archive corpus_ (integration tests): `..` and absolute
      paths, symlinks and hardlinks escaping the root, a symlink planted
      before a later regular file, device nodes and FIFOs, setuid/setgid
      bits (must be stripped), very long names, and an archive expanding
      past a size cap (derived from the build's `Installed-Size` × margin).
      Nothing may appear outside `.staging/`.
    - _Hostile server_ (the 2.6 fixture server): checksum mismatch,
      truncated body, a `Range` response that ignores the range, an HTTPS
      → HTTP redirect (refused), oversized pointer/sums/manifest files
      (reads capped), stalled connections (timeouts, no hang), and a
      changed archive under the same build id. The store must stay
      consistent and the cache must not keep a bad file.
    - _Filesystem and processes:_ created dirs and lock files are `0700`/`0600`;
      a lock dir owned by another user is refused; no `/tmp` fallback;
      the prefix path or `$HOME` being a symlink is handled deliberately
      (tested); child processes inherit only the scout lock fd meant for them;
      `--json`, run logs and `tracing` output contain no credentials (there
      are none to leak, but env values are logged unredacted, as in lootrs,
      and USAGE.md must say so).
  - [ ] **5.5b Performance.** Budgets are measured on the dev machine, with
        generous CI limits to catch regressions without flaking. Record
        baselines in `docs/AUDIT.md`.
    - _Startup_ (`hyperfine`, warm cache): `sruman --help`, `-Q`, `-P`, `-Qi sniper`,
      and `-Pk` with 50 profiles. Target < 20 ms each. No network on
      `-Q`/`-P`/`-X`.
    - _Network:_ the fixture server counts requests. A no-op `-Sy` (same
      `update-timestamp.txt`) makes **1** request. A full `-Sy` makes
      O(runtimes), not O(builds). An interrupted download resumes with
      `Range` instead of starting over.
    - _Install pipeline_ (`criterion` + a 200 MB synthetic archive):
      download→hash→decompress→extract throughput, and peak RSS
      (`/usr/bin/time -v`) under 64 MiB, i.e. streaming with no whole-archive
      buffers. Check the cost of two passes over the archive (hash, then
      extract) against a single tee'd pass, and switch if it's significant.
    - _Verify:_ `-Qkk` on a sniper build, timed and bounded in memory, and
      parallel across files if that helps.
    - _Launch overhead:_ the time from `sruman -X` to the child's exec
      (fake runtime that exits immediately), target < 10 ms beyond the
      runtime itself.
    - _Size:_ release binary size (`cargo bloat`) and dependency count
      (`cargo tree -e normal | wc -l`) for each feature level, with budgets
      in AUDIT.md.
  - [ ] **5.5c Usability and bugs.**
    - _Snapshot tests_ (`insta`) for every row of the CLI tables: help text,
      transaction summaries, `-Qi`/`-Pi` output, every user-facing error,
      and the `--json` schemas. Run with `NO_COLOR` and with forced color.
      Every error must name a fix: a command to run, or a file and field.
    - _Docs ↔ code:_ a test parses the operation tables in `docs/USAGE.md`
      and checks that each operation/modifier exists in clap and vice versa,
      and that every key in `examples/config.toml` exists in `FileConfig`
      and every field in `examples/games.toml` exists in `Game`.
    - _Exit codes:_ a matrix test covering each code path for 0/1/2/3/4.
    - _Property tests_ (`proptest`): `BuildId` ordering is total and
      consistent with parse/display; `RuntimeRef` round-trips; dependency
      order puts every dependency first; `clean` never removes a pinned,
      locked, linked or `current` build; manifest diff applied to old gives
      new.
    - _Crash consistency:_ cancel or kill at every `Progress` phase of an
      install/upgrade, then reopen the store. No half-installed build is
      visible, `current` points at a complete build, and `.staging` is
      cleaned on the next run.
    - _Concurrency:_ two `sruman -S sniper` at once (one waits or no-ops),
      `-Sc`/`-R` while a game runs on that build (skipped, exit 4 when
      forced without `-dd`), `-Pdd` on a prefix in use, two profiles
      running in one shared prefix, and starting a game that's already
      running (exit 4).
    - _Mutation testing_ (`cargo-mutants` on `libsruman`, scheduled):
      surviving mutants in `store`, `install`, `catalog`, `version`, `model`,
      `prefix` get tests or are recorded as accepted equivalents.
    - _Coverage:_ `libsruman` ≥ 85% lines, checked in `test.yml`.
    - _Canary:_ the `#[ignore]`d real-server test runs weekly in `audit.yml`
      (`-Sy`, `-Si` for every catalog runtime, plus a download of the
      smallest archive), so a layout change on Valve's side is noticed
      before users hit it.
- [ ] **5.6** Fix or record every 5.5 finding. Rerun the full audit job
      and all §8 gates green at the three feature levels, and update the
      baselines in `docs/AUDIT.md`.
- [ ] **5.7** Tag 0.1.0. Publish `libsruman`, then `sruman`. Consider an
      AUR `PKGBUILD` (fitting for a paru-like tool).

---

## Downstream integration (later; recorded here to check the API against)

- **lootrs**: add a `slr` feature to `liblootrs` → `libsruman`. `Profile`
  gains `slr: Option<libsruman::model::SlrSection>`, valid on `native` (a
  warning elsewhere). Its `prefix` field, today ignored for native, then means
  the same thing as in sruman. `launch::prepare`'s native runner calls
  `libsruman::launch::wrap`, and `lootrs check` calls
  `libsruman::launch::preflight`. Runtime management stays in `sruman`, or
  lootrs reuses `Transaction` behind its own command.
- **gaki**: it gets `[slr]` through `liblootrs::model` (its D9). Native Steam
  games can run on a sruman-managed runtime behind a `slr` feature, with
  installs via `spawn_blocking` and gaki's `reqwest` as the `Fetch` (D2).
  Later, Proton could launch on a sruman-managed sniper instead of Steam's copy.
- **One store for all three**: `$XDG_DATA_HOME/sruman/runtimes` and the
  flock protocol mean a runtime downloads once and no tool removes a build
  another is running.

While building Phases 1–3, check that lootrs could integrate without
`libsruman` depending on anything of lootrs', without enabling a default
feature it doesn't want, and without copying code. If not, fix the API here
first.

## Open questions

None block implementation. Already resolved by the spike: scout's
dependency (D7), the private home (D11), and `-Qkk` (Valve's `mtree.txt.gz`).

- The D5 hardening (cross-checking manifests against the signed apt index)
  is deferred to after 0.1.0, unless you want it earlier.
- Sniper was checked with a real game (Buckshot Roulette). A scout-era game
  (e.g. a free one such as SuperTuxKart or Extreme Tux Racer from a Steam or
  GitHub build) is still worth a manual run in 5.4, but it doesn't block
  anything.
