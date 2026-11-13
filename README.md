# rust-template

Template for rust projects. Configures:

- typos checks via `.typos.toml`: run `typos .`
- empty `LICENSE` file
- basic `deny.toml`: run `cargo deny check licenses`
- ready to use `CHANGELOG` file
- minimalistic workspace-level `Cargo.toml`
- basic `.gitignore` file
- empty `examples/` directory for config/theme/etc examples
- empty `docs/` directory for user and agent docs
- weekly dependabot scans via `.github/dependabot.yml`
- CI/CD jobs handled by GitHub Actions via `.github/workflows`
    - three build jobs: minimal (`--no-default-features`), normal, full (`--all-features`) via `build.yml`
    - deny check job via `deny.yml`
    - automated deploy to GitHub releases and crates.io on new tag pushes
    - automated crates.io `doc` build checks via `doc.yml`
    - formatters checks: fmt, clippy-minimal (`--no-default-features`), clippy-normal, clippy-full (`--all-features`) via `lint.yml`
    - typos check via `typos.yml`
    - automated tests checks with `codecov` coverage reports:  via `test.yml`

## Requires manual changes

Look for `TODO` markers and replace with your data.

- `test.yml`: specify projects for `codecov` job
- `deploy.yml`: binary and library crates paths to build and publish

## Required environment variables

- `CARGO_REGISTRY_TOKEN` for `deploy` job to crates.io
- `CODECOV_TOKEN` for code coverage reports on `test` job
