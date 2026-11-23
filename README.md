# sruman

**Steam Runtime Manager**: a package manager for the
[Steam Linux Runtime](https://gitlab.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/docs/container-runtime.md)
that works without Steam, and a launcher that runs native Linux games inside
those runtimes through lootrs-style games, profiles and prefixes.

```sh
wine game.exe                         # Windows game in a wine prefix
sruman execute -p scout ./game.x86_64 # native game in a Steam Linux Runtime "prefix"
```

> **Status: early development.** The commands below work; custom runtimes
> are still to come. The design is in
> [`docs/PLAN.md`](docs/PLAN.md).

Documentation: [`docs/USAGE.md`](docs/USAGE.md) (commands, games, profiles,
prefixes) and [`docs/RUNTIMES.md`](docs/RUNTIMES.md) (where runtimes come
from, how they're checked, troubleshooting). Examples are in
[`examples/`](examples/).

## What it does

- **Runtimes as packages.** Installs, upgrades, pins, verifies and removes
  builds of `scout`, `soldier`, `sniper` and `steamrt4` from Valve's
  public image server (`repo.steampowered.com`), checked against Valve's
  `SHA256SUMS`. It shows which Debian packages change between builds, and
  several builds can be installed side by side. Use **`steamrt4`**, the
  latest, for native games; the older ones are only for games and Protons
  that need them.
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
- **Steam on sruman's runtimes.** `sruman steam link` replaces Steam's own
  runtimes (its client's `steamrt3c` and the game runtimes) with links to
  the same builds, so Steam runs on sruman's; `sruman steam relocate`
  moves Steam's library, caches and logs out of its folder (to
  `steam_library`, `~/.cache/steam`, `~/.local/state/steam`), so the folder
  keeps only the client. Both undo (`steam unlink`, `steam relocate
  --undo`). See [Steam](docs/USAGE.md#steam).
- **Library first.** `libsruman` holds all the logic, so
  [lootrs](https://github.com/Gigas002/lootrs) and
  [gaki](https://github.com/Gigas002/gaki) can use it as an optional feature.

## Usage

```sh
sruman upgrade --sync                     # refresh the index and upgrade installed runtimes
sruman install steamrt4                   # install a runtime (the latest)
sruman install steamrt4=4.0.20261021.265997 # install a specific build
sruman search steamrt4 / info steamrt4    # search / show a runtime
sruman runtimes / outdated / verify       # installed, upgradable, check their files
sruman remove soldier                     # remove
sruman import ~/.local/share/Steam/steamapps/common/SteamLinuxRuntime_4 --link   # adopt Steam's copy

sruman run hollow-knight                  # run a game from games.toml
sruman execute -p steamrt4 ./game.x86_64  # run an executable with a profile
sruman profiles list / show steamrt4      # profiles
sruman profiles new indie -r steamrt4 --mode overlay   # a profile on your system, no container
sruman list / check                       # games; check games and profiles
sruman run hollow-knight --wait           # run in the foreground, with its output and exit code
sruman logs hollow-knight                 # the latest run log
sruman status / stop hollow-knight        # running games / stop one
sruman shell -p steamrt4                  # a shell in the profile's runtime and prefix
sruman provide-umu --appid 4183110        # hand steamrt4 to umu (UMU_FOLDERS_PATH, no umu downloads)
sruman steam link                         # Steam on sruman's runtimes (links in place of Steam's copies)
sruman steam relocate                     # Steam's library, caches and logs out of its folder
sruman steam status / verify / clean      # where everything is; check; drop builds Steam wrote over
sruman doctor                             # check the system (user namespaces, runtime self-test)
```

## Requirements

- Linux, x86_64 (arm64 later for `steamrt4`).
- Unprivileged user namespaces, which most distributions enable. The container
  runtimes use pressure-vessel, which Valve ships inside each runtime.
  Overlay mode (`mode = "overlay"`) runs games without a container.
- Steam is **not** required. Games that insist on a running Steam client still
  need Steam (or gaki); the `steam` commands only matter if you use Steam.

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
