# sruman

**Steam Runtime Manager**: a pacman/paru-style package manager for the
[Steam Linux Runtime](https://gitlab.steamos.cloud/steamrt/steam-runtime-tools/-/blob/main/docs/container-runtime.md)
that works without Steam, and a launcher that runs native Linux games inside
those runtimes through lootrs-style games, profiles and prefixes.

```sh
wine game.exe                         # Windows game in a wine prefix
sruman -p scout ./game.x86_64         # native game in a Steam Linux Runtime "prefix"
```

> **Status: early development.** Nothing below works yet. The design is in
> [`docs/PLAN.md`](docs/PLAN.md); this README describes where it's going.

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
- **Games list.** `games.toml` uses lootrs' format, so `sruman <game>` and
  `sruman <path>` both work.
- **Library first.** `libsruman` holds all the logic, so
  [lootrs](https://github.com/Gigas002/lootrs) and
  [gaki](https://github.com/Gigas002/gaki) can use it as an optional feature.

## Planned usage

```sh
sruman -Syu                          # refresh the index and upgrade installed runtimes (also: plain `sruman`)
sruman -S sniper                     # install a runtime
sruman -S sniper=3.0.20261021.266025 # install a specific build
sruman -Ss sniper / -Si sniper       # search / show a runtime
sruman -Q / -Qi sniper / -Qu / -Qk   # installed, info, upgradable, verify
sruman -R soldier                    # remove
sruman -U ~/.local/share/Steam/steamapps/common/SteamLinuxRuntime_sniper --link   # adopt Steam's copy

sruman hollow-knight                 # run a game from games.toml
sruman -p scout ./game.x86_64        # run a path with a profile
sruman -P / -Pi scout / -Pk          # profiles: list, show, check
sruman -G / -Gk                      # games: list, check
sruman -Xs / -Xk hollow-knight       # running games / stop one
sruman -D                            # check the system (user namespaces, runtime self-test)
```

## Requirements

- Linux, x86_64 (arm64 later for `steamrt4`).
- Unprivileged user namespaces, which most distributions enable. The container
  runtimes use pressure-vessel, which Valve ships inside each runtime.
  `scout-ldlp` runs without a container.
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
