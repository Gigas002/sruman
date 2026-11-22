# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

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

- Overlay mode (`[slr] mode = "overlay"`): the game runs on the host, with
  the runtime's libraries on `LD_LIBRARY_PATH` where the host's are missing
  or older. glibc and the GPU stack always come from the host, and the
  host's glibc must be at least the runtime's. `[slr.lib_overrides]` picks
  `runtime`, `host` or `newest` per library.

- Project setup: `libsruman` and `sruman` crates, GPL-3.0-or-later license,
  CI workflows, and the implementation plan (`docs/PLAN.md`) with the
  verified Steam Linux Runtime findings.
