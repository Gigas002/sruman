# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-11-23

The first release.

### Security

- Archives can't unpack to more than twice their packages' `Installed-Size`
  (20 times the archive without a manifest, at least 512 MiB) or a million
  entries; device nodes stay refused, as before.
- Redirects from HTTPS to plain HTTP are refused.
- sruman's directories are created `0700` and its lock files `0600`; a
  directory owned by someone else, or writable by others, is refused.
- No `unsafe` code in either crate, no process started through a shell, and
  no `unwrap`, `expect` or unchecked indexing in the code that parses
  downloaded data (enforced by lints and a policy test).

### Fixed

- Extracting uses at most two decoder threads: it was as fast, but took
  about 700 MiB on a 12-core machine instead of 190 MiB.
- `profiles list` reads `/proc` once instead of once per profile.
- A download cancelled while extracting is reported as cancelled.
- `import` no longer prints the path twice in its errors.
- `show` shows the profile's prefix also when the game can't start yet (its
  runtime isn't installed), instead of "None (real home)".

### Changed

- libgameprofiles comes from crates.io (`0.1`) instead of its git master
  branch; dependencies updated.
- Profiles use the [libgameprofiles](https://github.com/Gigas002/libgameprofiles)
  format and loader, shared with lootrs and gaki, instead of sruman's own
  copy: a profile file now needs `runner = "slr"` next to its `[slr]`
  section. `model::validate_profile` replaces `Profile::validate`, and
  `model::new_profile` builds a profile for a runtime. A bad `slr.runtime` is
  a validation error instead of a load error.

- `scout-ldlp` is no longer a runtime of its own: use `scout` with
  `mode = "overlay"`, which runs on scout's bundled LD_LIBRARY_PATH runtime
  without soldier.

### Added

- `docs/USAGE.md` (command reference, games, profiles, prefixes, exit codes,
  logs), `examples/config.toml`, and `docs/RUNTIMES.md` sections on
  channels, apt vs images, importing Steam's and umu's runtimes,
  troubleshooting and manual release checks. Tests keep the docs, examples
  and CLI in step.

- The `sruman` binary: runtime commands (`sync`, `install`, `upgrade`,
  `outdated`, `remove`, `search`, `info`, `runtimes`, `builds`, `files`,
  `verify`, `clean`, `import`), games and running (`run`, `execute`,
  `shell`, `status`, `stop`, `killall`, `list`, `show`, `check`, `logs`,
  `init`), `profiles list|show|new|edit|edit-games|delete` and `doctor`.
  Detached runs stay under a supervisor that writes a run log per game;
  `--json` for scripts; exit codes as in lootrs. A runtime missing at launch
  is offered for install (D12).

- `sruman provide-umu <rt> | --appid <id>` and `libsruman::umu`: an
  installed build as umu-launcher's runtime folder
  (`$XDG_DATA_HOME/sruman/umu/<variant>/`, a symlink view with umu's marker),
  so umu runs sruman's runtime instead of downloading its own. `--json`
  gives lootrs the folders path and the build's hold file.

- Overlay mode (`[slr] mode = "overlay"`): the game runs on the host, with
  the runtime's libraries on `LD_LIBRARY_PATH` where the host's are missing
  or older. glibc and the GPU stack always come from the host, and the
  host's glibc must be at least the runtime's. `[slr.lib_overrides]` picks
  `runtime`, `host` or `newest` per library.

- Project setup: `libsruman` and `sruman` crates, GPL-3.0-or-later license,
  CI workflows, and the implementation plan (`docs/PLAN.md`) with the
  verified Steam Linux Runtime findings.
