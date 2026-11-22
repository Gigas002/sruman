# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Profiles use the [libgameprofiles](https://github.com/Gigas002/libgameprofiles)
  format and loader, shared with lootrs and gaki, instead of sruman's own
  copy: a profile file now needs `runner = "slr"` next to its `[slr]`
  section (`mode = "overlay"` isn't supported yet). `model::validate_profile` replaces `Profile::validate`, and
  `model::new_profile` builds a profile for a runtime. A bad `slr.runtime` is
  a validation error instead of a load error.

### Added

- Project setup: `libsruman` and `sruman` crates, GPL-3.0-or-later license,
  CI workflows, and the implementation plan (`docs/PLAN.md`) with the
  verified Steam Linux Runtime findings.
