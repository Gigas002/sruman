# sruman

**Steam Runtime Manager**: a package manager for the
[Steam Linux Runtime](https://gitlab.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/docs/container-runtime.md)
that works without Steam, and a launcher that runs native Linux games inside
those runtimes through lootrs-style games, profiles and prefixes.

```sh
wine game.exe                         # Windows game in a wine prefix
sruman execute -p scout ./game.x86_64 # native game in a Steam Linux Runtime "prefix"
```

> **Status: early development.** The commands below work; `provide-umu`
> and custom runtimes are still to come. The design is in
> [`docs/PLAN.md`](docs/PLAN.md).

## What it does

- **Runtimes as packages.** Installs, upgrades, pins, verifies and removes
  builds of `scout`, `soldier`, `sniper` and `steamrt4` from Valve's
  public image server (`repo.steampowered.com`), checked against Valve's
  `SHA256SUMS`. It shows which Debian packages change between builds, and
  several builds can be installed side by side.
- **Prefixes for native games.** A profile picks a runtime and an optional
  prefix directory. The game then gets the prefix as its home, so saves and
  configs stay there instead of in your real home, the way a wine prefix keeps
  `drive_c/users`. Several profiles can share one prefix.
- **Games list.** `games.toml` uses lootrs' format, so `sruman run <game>`
  and `sruman execute <path>` both work, with lootrs' command names.
- **Shared profiles.** Profiles are in the
  [libgameprofiles](https://github.com/Gigas002/libgameprofiles) format that
  lootrs and gaki use too: a sruman profile has `runner = "slr"` and an
  `[slr]` section.
- **Library first.** `libsruman` holds all the logic, so
  [lootrs](https://github.com/Gigas002/lootrs) and
  [gaki](https://github.com/Gigas002/gaki) can use it as an optional feature.

## Usage

```sh
sruman upgrade --sync                     # refresh the index and upgrade installed runtimes
sruman install sniper                     # install a runtime
sruman install sniper=3.0.20261021.266025 # install a specific build
sruman search sniper / info sniper        # search / show a runtime
sruman runtimes / outdated / verify       # installed, upgradable, check their files
sruman remove soldier                     # remove
sruman import ~/.local/share/Steam/steamapps/common/SteamLinuxRuntime_sniper --link   # adopt Steam's copy

sruman run hollow-knight                  # run a game from games.toml
sruman execute -p scout ./game.x86_64     # run an executable with a profile
sruman profiles list / show scout         # profiles
sruman profiles new indie -r sniper --mode overlay   # a profile on your system, no container
sruman list / check                       # games; check games and profiles
sruman run hollow-knight --wait           # run in the foreground, with its output and exit code
sruman logs hollow-knight                 # the latest run log
sruman status / stop hollow-knight        # running games / stop one
sruman shell -p scout                     # a shell in the profile's runtime and prefix
sruman doctor                             # check the system (user namespaces, runtime self-test)
```

## Requirements

- Linux, x86_64 (arm64 later for `steamrt4`).
- Unprivileged user namespaces, which most distributions enable. The container
  runtimes use pressure-vessel, which Valve ships inside each runtime.
  Overlay mode (`mode = "overlay"`) runs games without a container.
- Steam is **not** required. Games that insist on a running Steam client still
  need Steam (or gaki).

## Files

| What                     | Default location               |
| ------------------------ | ------------------------------ |
| Settings, games, profiles | `~/.config/sruman/`            |
| Runtimes, prefixes        | `~/.local/share/sruman/`       |
| Download cache, index     | `~/.cache/sruman/`             |
| Logs                      | `~/.local/state/sruman/logs/`  |

## Building

```sh
cargo build --release                       # sruman + libsruman with default features
cargo build --release --no-default-features # no network code: manage and run existing runtimes only
```

## License

GPL-3.0-or-later. The Steam Linux Runtime itself is Valve's and is downloaded
from Valve's servers; sruman doesn't redistribute it.
