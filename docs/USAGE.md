# Using sruman

The command reference, and how games, profiles and prefixes fit together.
[`RUNTIMES.md`](RUNTIMES.md) covers the runtimes themselves: where they come
from, how they're checked, and troubleshooting.

Every command has `--help`. Options that work everywhere: `-y`/`--yes`
(answer every prompt with its default), `-q`/`--quiet`, `-v`/`-vv` (log to
stderr), `--color auto|always|never` and `--config <file>`. Commands that
print data take `--json` (the `json` feature, on by default).

## Commands

### Runtimes

These need the `download` feature (on by default) unless noted.

| Command | What it does |
| ------- | ------------ |
| `sruman sync` | Refresh the sync index: one small request per server directory when nothing changed. `--force` refetches everything. |
| `sruman install` | Install runtimes with their dependencies: `sruman install steamrt4` (the latest; use it unless a game needs an older one), or a build: `steamrt4=4.0.20261021.265997`. `--needed` skips installed ones, `--download-only` only fills the cache. |
| `sruman upgrade` | Upgrade installed runtimes (all, or the named ones) to their channel's build. `--sync` refreshes the index first, `--allow-downgrade` follows a channel that went back, `--download-only` only downloads. Pinned profiles keep their build. |
| `sruman outdated` | Installed runtimes with a newer build in the index. |
| `sruman search` | Search runtimes by name and description; `--packages` searches their Debian packages (`search --packages libvulkan`). |
| `sruman builds` | List a runtime's builds on the server, for pinning. |
| `sruman clean` | Remove old builds (keeping `keep_versions` per runtime, the current one, and builds that profiles pin, that are linked or in use) and the cached archives of builds that aren't installed. `--all` removes every cached archive. |
| `sruman remove` | Remove runtimes (all their builds) or single builds (`steamrt4=<build>`). It refuses while profiles still use the build; `--force` skips that check, but a build in use by a running game is never removed. Works without `download`. |
| `sruman info` | A runtime's build, channel, sizes, dependencies and the profiles using it; `--packages` lists its packages. Works without `download` for installed runtimes. |
| `sruman runtimes` | Installed runtimes and builds. Works without `download`. |
| `sruman files` | An installed build's files, from Valve's file list. Works without `download`. |
| `sruman verify` | Check installed builds (all without arguments) against Valve's file lists: every file, directory and link present, sizes matching; `--full` also checks every file's SHA-256. Works without `download`. |
| `sruman import` | Install from a local archive (`SteamLinuxRuntime_4.tar.xz`, needs `download`) or a runtime directory (Steam's or umu's copy). `--link` registers a directory in place instead of copying it; sruman never deletes a linked build's files. |
| `sruman provide-umu` | Make a runtime umu's folder (`$XDG_DATA_HOME/sruman/umu/<variant>/`, a view of the build), so umu runs it and never downloads its own: `provide-umu steamrt4`, or `--appid 4183110` as Proton's `toolmanifest.vdf` names it. Point umu's `UMU_FOLDERS_PATH` (lootrs: `umu_folders`) at sruman's data directory to use it. |
| `sruman steam status` | Steam's runtimes (its client's `steamrt3c`, and the game runtimes in each library): linked to which build, or Steam's own copy. Works without `download`. |
| `sruman steam link` | Replace Steam's runtimes (all, or the named ones) with links to the same builds in the store, importing Steam's copy of a build sruman doesn't have; see [Steam](#steam). Steam must not run. Works without `download`. |
| `sruman steam unlink` | Put copies of the builds back in place of the links, so Steam has its own runtimes again. Steam must not run. Works without `download`. |
| `sruman doctor` | Check unprivileged user namespaces, bubblewrap and the installed runtimes (needs the `diag` feature, on by default). |

### Games and profiles

| Command | What it does |
| ------- | ------------ |
| `sruman run` | Run a game from `games.toml`: `sruman run hollow-knight -- --extra-arg`. `-p` overrides its profile. |
| `sruman execute` | Run an executable once with a profile: `sruman execute -p steamrt4 ./game.x86_64`. A target with a `/` or a leading `~` is a path; a bare name is a program in the runtime's `PATH` (`execute -p steamrt4 glxgears`). Without `-p`, `default_profile`. |
| `sruman shell` | An interactive shell in a profile's runtime and prefix (pressure-vessel's shell in container mode, `$SHELL` in overlay mode). |
| `sruman list` | The games in `games.toml`. |
| `sruman show` | A game's executable, profile, runtime build, prefix and environment, as it would launch. |
| `sruman check` | Check games and profiles (all without arguments) without launching: names, files, runtimes, wrappers. |
| `sruman status` | Running games, executables and shells, with their PIDs and logs. |
| `sruman stop` | Stop games, profiles' sessions or PIDs: SIGTERM, then SIGKILL after `stop_timeout_secs`. |
| `sruman killall` | Stop everything sruman started. |
| `sruman logs` | Print a game's latest run log; `--path` prints its path. |
| `sruman init` | Create or update a profile's prefix without launching anything. |
| `sruman profiles list` | Profiles, implicit ones included, with their runtime, mode, prefix and whether they're in use. |
| `sruman profiles show` | A profile: runtime build, mode, prefix, and which other profiles share the prefix. |
| `sruman profiles new` | Create a profile for a runtime (`-r`/`--runtime`), then its prefix: `profiles new indie -r steamrt4 [--mode overlay] [--prefix ~/pfx/indie]`. Container profiles get `<prefix_root>/<name>` without `--prefix`; overlay profiles get no prefix. |
| `sruman profiles edit` | Edit a profile in `$VISUAL`/`$EDITOR`, then check it. |
| `sruman profiles edit-games` | Edit `games.toml` the same way. |
| `sruman profiles delete` | Delete profiles; `--prefix` deletes their prefixes too, unless another profile shares one or it's in use. |

`run`, `execute` and `shell` start the game detached and return: its output
goes to the run log. `--wait` keeps it in the foreground, prints its output
and exits with its exit code (128 + the signal if it was killed). `--debug`
turns on the runtime's verbose logging for that launch.

A runtime that isn't installed when a game starts is offered for install on
a terminal (`auto_install = true` installs it without asking); otherwise
sruman stops with exit code 2 and the `sruman install` command to run.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success. |
| 1 | Something failed (download, checksum, filesystem), or a prompt was declined. |
| 2 | Invalid input: unknown command or option, a bad name or build id, a broken config, games or profile file, a runtime or game that doesn't exist. |
| 3 | Not running: `stop`, `logs` for something that isn't running or never ran. |
| 4 | Busy: the store or a prefix is locked by another sruman, a build is in use by a running game, or the game is already running. |

With `--wait`, the game's own exit code instead.

## Files

| What | Where |
| ---- | ----- |
| Settings | `~/.config/sruman/config.toml` ([example](../examples/config.toml)) |
| Games | `~/.config/sruman/games.toml` ([example](../examples/games.toml)) |
| Profiles | `~/.config/sruman/profiles/<name>.toml` ([examples](../examples/profiles/)) |
| Runtimes | `~/.local/share/sruman/runtimes/<name>/<build>/` |
| Prefixes | `~/.local/share/sruman/prefixes/<profile>/` (`prefix_root`) |
| umu views | `~/.local/share/sruman/umu/<variant>/` |
| Steam | `steam_dir` in `config.toml`, else `~/.steam/root`, else `~/.local/share/Steam` |
| Sync index | `~/.cache/sruman/sync/` |
| Download cache | `~/.cache/sruman/pkg/` |
| App logs | `~/.local/state/sruman/logs/` (daily, `log_keep` kept) |
| Run logs | `~/.local/state/sruman/logs/{games,exec,init}/<name>/` |
| Locks | `$XDG_RUNTIME_DIR/sruman/`, else the state directory |

The XDG variables move them as usual. sruman creates its directories `0700`
and its lock files and run logs `0600`, and refuses a directory that belongs
to another user or that others can write.

## Games

`games.toml` holds `[[game]]` entries in lootrs' format: `name`, `exe`,
optional `profile`, `args`, `cwd` and `env`. A game without `profile` uses
`default_profile` from `config.toml`. `env` is merged over the profile's,
and `args` come before the arguments given on the command line.

`sruman run <name>` only runs games from this file, and `-p` runs one with
another profile. `sruman execute` runs anything else: a path, or a program
the runtime has. A bare name is never a file in the current directory: write
`./game` for that, as in a shell.

## Profiles and prefixes

A profile is a libgameprofiles file with `runner = "slr"` and an `[slr]`
section naming the runtime (`runtime = "steamrt4"`, or `"steamrt4=<build>"`
to pin a build). Prefer `steamrt4`, the latest runtime; pick `sniper`,
`soldier` or `scout` only for a game that doesn't run on it. Every runtime also has an **implicit profile** of the same
name, with no prefix, so `sruman execute -p steamrt4 ./game` works without any
file.

Two choices are independent:

- **The home.** With `prefix` set, the game gets `<prefix>/home` as its
  home: saves, configs and caches land there, not in your real home, the way
  a wine prefix keeps `drive_c/users`. Without it, the game sees your real
  home.
- **The libraries** (`[slr] mode`). `container` (default) runs the game in
  pressure-vessel with the runtime's `/usr`. `overlay` runs it on your
  system, without a container, with the runtime's libraries added on
  `LD_LIBRARY_PATH` where your system's are missing or older; glibc and the
  GPU drivers always come from the host. `[slr.lib_overrides]` picks
  `runtime`, `host` or `newest` per library in overlay mode, e.g.
  `"libSDL2-2.0.so.0" = "host"` for a newer SDL.

**Shared prefixes.** Several profiles may name the same prefix (one game's
saves for two runtimes, say). Launches take a shared lock on it, so they can
run at once; `profiles delete --prefix` and anything else that rewrites the
prefix take it exclusively and wait for none: they fail with exit code 4
while a game uses it.

Neither mode is a security sandbox. A container still sees the GPU, D-Bus,
the display sockets and the paths shared with it, and pressure-vessel
exposes the host Steam client library, so a game can reach a running Steam.

**Wrappers.** `wrappers = ["gamemoderun", "mangohud"]` run in front of
everything, outside the container. MangoHud works without being in the
runtime: pressure-vessel imports the host's Vulkan layers.

## Logs

Each launch writes a run log: the game and profile as loaded, the runtime
build, the exact command line, **the environment sruman set (unredacted)**,
the system, then the game's output. Don't put secrets in a profile's or
game's `env`; they end up in the run log (which is `0600`). `-v` logs to
stderr as well, and the daily app log under `logs/` records what each
command did.

## Steam

Steam keeps its own copy of every runtime: one for its client
(`steamrt64/pv-runtime/steam-runtime-steamrt`, the `steamrt3c` runtime) and
the game runtimes as apps in each library
(`steamapps/common/SteamLinuxRuntime_4`, `_soldier`, `_sniper`, and scout's
`SteamLinuxRuntime`). Steam checks its client files at every start and puts
a missing runtime back, so deleting them doesn't last.

`sruman steam link` replaces each with a symlink to the same build in
sruman's store: Steam follows the link and finds the files it expects, and
runs its client and games on sruman's runtimes. A build sruman doesn't have
yet is imported from Steam's copy (nothing is downloaded); every build is
checked against its file list before Steam's copy is replaced.

- A link names one build, not `current`: `upgrade` doesn't move a runtime
  under Steam, and `clean` and `remove` keep the builds Steam links to
  (`remove` names the link; `--force` removes the build anyway).
- When Steam updates a runtime, it writes the new build through the link
  into the old one. `steam status` shows that as "updated by Steam";
  `steam link` then imports the new build, repoints the link, and tells you
  how to reinstall the old one.
- `steam unlink` copies the builds back, which undoes it.

Quit Steam before `link` and `unlink` (sruman refuses while it runs, exit
code 4). sruman finds Steam through `~/.steam/root`; set `steam_dir` for
another one.

## Using sruman from lootrs and gaki

lootrs and gaki read the same profiles (libgameprofiles). gaki runs native
games with `slr` profiles through `sruman execute`. lootrs doesn't call
sruman: to run its umu profiles on sruman's runtime, run
`sruman provide-umu steamrt4` once and set `umu_folders =
"~/.local/share/sruman"` in lootrs' `config.toml` (umu's
`UMU_FOLDERS_PATH`); see D15 in [`PLAN.md`](PLAN.md).
