# Audit

The pre-release audit (PLAN step 5.5), in three parts: security (5.5a),
performance (5.5b), and usability and bugs (5.5c). Most of it is automated:
lints and tests re-run it with the rest of the suite, and §5 lists each
part's local command. Each finding says what was found, how serious it is,
and what was done: **fixed** in the audit, **accepted** with the reason, or moved to a
**follow-up** PLAN step.

Severity: **high** (code execution, or writes outside sruman's directories
and the user's prefixes), **medium** (a crash, a hang or an unbounded
allocation from outside input, a corrupted store), **low** (hardening; needs
an unlikely setup or an attacker who already controls something trusted).

Audited on 2026-11-23, at the commits of branch `feature/release-0.1.0`.

## 1. Security (5.5a)

### 1.1 Static rules

Enforced on every build, not checked once:

| Rule | How | Status |
| ---- | --- | ------ |
| No `unsafe` | `#![forbid(unsafe_code)]` in both crates and the binary; `rustix` covers locks, signals and `setsid`. `tests/policy.rs` also rejects the word in source. | OK |
| Processes start in known places | `clippy.toml` bans `std::process::Command::new`. Allowed in `diag` (doctor's tools) and `app/supervisor` (games, shells), and with a stated reason in `HostLibs::system` (`ldconfig -p`) and `run_editor` (`$VISUAL`/`$EDITOR`, split on whitespace). | OK |
| No shell | `tests/policy.rs` rejects `"-c"` in the source: commands are argv vectors. `sruman shell` starts an interactive shell without `-c`. | OK |
| No panics on outside data | `fetch`, `sync`, `manifest`, `install`, `store::import`, `mtree` and `version` deny `unwrap`, `expect` and indexing outside tests. Seven slicing sites (read buffers, mtree escapes, the not-attempted list) used `[..]` where the bound was right but unchecked; they use `get` now. | Fixed (low) |
| Dependencies | `cargo deny check`: advisories, bans, licenses and sources all pass. | OK |

### 1.2 Untrusted input

The server, the archives and the files a user imports are outside data.

| Area | Finding | Severity | Status |
| ---- | ------- | -------- | ------ |
| Archive size | Extraction had no limit on how much an archive unpacks to: a small archive with a matching checksum (a compromised server or mirror, or a local archive given to `import`) could fill the disk or its inodes. It now stops past twice the packages' `Installed-Size` (20 times the archive when there's no manifest, never below 512 MiB) or a million entries, checking each entry's size before writing it. Valve's builds are 0.5 to 0.9 times their `Installed-Size`. | medium | Fixed (`ddf8665`) |
| Archive entries | Only regular files, directories and relative symlinks that stay inside are accepted; absolute and `..` paths, hardlinks, writing through or over a symlink, FIFOs and device nodes fail the install. Setuid, setgid and sticky bits are dropped. New tests: char and block devices, mode bits, a GNU long-name record (over 100 bytes) carrying `..`, a header claiming 1 TiB. Nothing lands outside `.staging/`. | — | OK |
| HTTPS → HTTP redirect | ureq followed a redirect from HTTPS to plain HTTP, so a mirror redirect could put the archive and its `SHA256SUMS` on an unprotected connection. Refused now (the redirect history is checked); plain HTTP stays possible only for a `base_url` on localhost. Valve's server doesn't redirect. | low | Fixed (`5a22310`) |
| Small files | Pointers, `SHA256SUMS`, os-release, manifests and the build listing are read with a 16 MiB cap, with or without `Content-Length`. | — | OK |
| Stalled server | A connection that stops sending fails after 60 s per read instead of hanging; a test checks both a stall before the body and one partway through. | — | OK |
| Bad downloads | A checksum mismatch keeps nothing in the cache; a truncated body is an error, not a short file; a `Range` the server ignores restarts the download; an archive replaced under the same build id fails its checksum. All tested against the fixture server. | — | OK |
| Parsers | Property tests feed arbitrary text to every parser (build ids, runtime refs, channels, `SHA256SUMS`, os-release, autoindex, `Content-Range`, `manifest.dpkg`, mtree, `ldconfig -p`) and arbitrary or bit-flipped archives to the extractor: 50,000 cases each, no panics. | — | OK |
| Signatures | Images are checked by HTTPS and `SHA256SUMS` only; Valve doesn't sign them (the apt repository is GPG-signed, but sruman doesn't use it, see RUNTIMES.md). | low | Accepted |

### 1.3 Filesystem and processes

| Area | Finding | Severity | Status |
| ---- | ------- | -------- | ------ |
| Directory and lock modes | Lock files, the lock directory, logs, the cache and the store were created with the umask's modes (usually `0644`/`0755`), and nothing checked who owned the lock directory. Now `paths::private_dir` creates them `0700` and refuses one owned by another user or writable by others; lock files are `0600`. `$XDG_RUNTIME_DIR` is already private, so this mostly matters for the fallback under the state directory. | low | Fixed (`57a3f89`) |
| `/tmp` | Never used: locks fall back to `$XDG_STATE_HOME/sruman/run` without `$XDG_RUNTIME_DIR`. | — | OK |
| Symlinked paths | Prefix locks are keyed by the canonical path, a prefix reached through a symlink can't be or contain the home, and links inside a prefix home aren't followed when it's reset or removed. Tested. | — | OK |
| Inherited fds | Rust opens every file close-on-exec; the scout lock and the prefix lock are the only fds a game inherits, on purpose, passed to pressure-vessel with `--pass-fd`. | — | OK |
| Logs | Run logs record the environment sruman set, unredacted, as lootrs does. They're `0600` in a `0700` directory, and USAGE.md says not to put secrets in `env`. sruman itself handles no credentials. | low | Accepted |
| Steam client in containers | pressure-vessel exposes the host's `steamclient.so` inside a container, so a game can reach a running Steam. That's how the Steam API works; USAGE.md says so. | low | Accepted |

## 2. Performance (5.5b)

Measured on the dev machine (12 threads, NVMe, `/tmp` on tmpfs), release
build, warm cache.

### 2.1 Startup

Median and 95th percentile over 60 runs, with 50 profiles:

| Command | Median | p95 | Budget |
| ------- | ------ | --- | ------ |
| `sruman --help` | 1.1 ms | 1.2 ms | 20 ms |
| `sruman runtimes` | 2.3 ms | 2.5 ms | 20 ms |
| `sruman profiles list` | 7.1 ms | 8.0 ms | 20 ms |
| `sruman list` | 7.0 ms | 7.6 ms | 20 ms |
| `sruman check` | 3.0 ms | 3.4 ms | 20 ms |
| `sruman info sniper` | 2.9 ms | 3.2 ms | 20 ms |
| `sruman status` | 6.2 ms | 6.8 ms | 20 ms |

Finding: `profiles list` read all of `/proc` once per profile to see which
ones were in use, 18.5 ms with 50 profiles. It reads it once now. (low,
**fixed** in `e65b0b6`.) The remaining ~4 ms in `list`, `status` and
`profiles list` is that one scan.

Only `sync`, `install`, `upgrade`, `outdated`, `builds` and the install
offered at launch create a network client; listing, checking and launching
never touch the network.

### 2.2 Install pipeline

An archive shaped like Valve's: 170 MiB of xz in 34 blocks of 24 MiB
(sniper has 33, steamrt4 29, read from their xz indexes), 803 MiB unpacked,
imported with `sruman import`:

| xz decoder threads | Time | Peak RSS |
| ------------------ | ---- | -------- |
| streaming (no threads) | 2.92 s | 19 MiB |
| 1 | 1.70 s | 108 MiB |
| **2** | **0.88 s** | **186 MiB** |
| 3 | 0.78 s | 247 MiB |
| 4 | 0.83 s | 349 MiB |
| 12 (old default: one per CPU, up to 16) | 0.87 s | about 700 MiB |

Finding: each decoder thread holds a whole block, compressed and unpacked,
so memory grew by about 70 MiB per thread while time stopped improving after
two (the single tar consumer is the bottleneck). On this machine an install
took 700 MiB for nothing. Now two threads at most. (medium: memory scaled
with the machine, not the input; **fixed** in `e65b0b6`.)

The PLAN's 64 MiB budget can only be met by streaming, which is 3.3 times
slower. Two threads are bounded (two blocks, whatever the archive's size),
so the budget is revised to **256 MiB**. **Accepted.** The download itself
streams (64 KiB buffers) and is hashed while it's written; extraction then
reads the archive once more from the page cache, which costs less than a
tee'd single pass would complicate resuming. **Accepted.**

### 2.3 Verify and launch

- `sruman verify --full` on Steam's steamrt4 (7,032 files, 679 MiB, linked
  with `import --link`): 1.65 s cold, 0.49 s warm, 16 MiB peak RSS.
  `verify` without `--full`: 0.11 s. Disk-bound when cold; hashing in
  parallel isn't worth it. **OK.**
- Launch overhead, from `sruman execute` to the game through a fake runtime
  whose `run` just execs: 5.6 ms against 3.3 ms for the runtime alone, so
  2.4 ms (budget 10 ms). **OK.**

### 2.4 Requests and size

- A sync with nothing new costs one request per server directory. A first
  sync of sniper costs 12: the timestamp, three pointers, and four per
  distinct channel build (sums, archive `HEAD`, os-release, manifest), however
  many builds the server keeps. An interrupted download resumes with
  `Range`. All tested.
- Release binary and normal dependencies:

  | Features | Binary | Crates | Budget |
  | -------- | ------ | ------ | ------ |
  | `--no-default-features` | 4,888 KiB | 69 | 6,144 KiB, 86 |
  | default (= all) | 8,589 KiB | 95 | 10,752 KiB, 119 |

  There's no `[profile.release]` with `strip` or LTO: `deploy.yml` strips
  the released binary, and so do packagers (`makepkg` by default).
  **Accepted.**

## 3. Usability and bugs (5.5c)

| Area | Finding | Severity | Status |
| ---- | ------- | -------- | ------ |
| Cancelled extraction | Cancelling during extraction came back as `InstallError::Extract(Cancelled)` instead of `InstallError::Cancelled`, and under load the decoder threads could turn the cancelled read into another error (one unreproduced failure of the new crash test). Both map to `Cancelled` now: the cancel flag decides. | low | Fixed (`2c68b0a`) |
| `import` errors | An error for a missing path printed the path twice (`/x: /x: No such file…`). | low | Fixed (`2c68b0a`) |
| `info` without downloads | `info <runtime>` for a runtime that isn't installed said only "is not installed"; in a build without the `download` feature it now says to `import` it. | low | Fixed (`2c68b0a`) |
| `show` without a runtime | `show <game>` printed `Prefix : None (real home)` for a game whose profile has a prefix when the launch couldn't be prepared, e.g. with the runtime not installed: misleading exactly when you're finding out what's wrong. It shows the profile's prefix now. Found while checking the dotfiles' sruman config. | low | Fixed |
| Hints without downloads | libsruman's store errors suggest `sruman install` even in a `sruman` built without `download`, where that command doesn't exist. Such builds are for packagers and embedding, and `--help` lists what's there. | low | Accepted |
| Sessions across configs | `status`, `in use` and `stop` find games by the `SRUMAN_*` variables in `/proc/<pid>/environ`, so two sruman configs of the same user (other `XDG_*` directories) see each other's games, and a profile name used in both counts as in use in both. Other users' processes aren't readable, and one user normally has one config; it showed up only in parallel tests, whose snapshots now mask live state. | low | Accepted |
| Docs | Writing USAGE.md showed that `execute` takes a path or a program in the runtime's `PATH`, not a game name (lootrs' `resolve_target` isn't used by sruman). USAGE.md says so; a test now keeps the docs' command and option lists in step with clap. | — | Fixed (docs) |

What runs on every `cargo test`:

- **Snapshots** (`insta`): every command's `--help`, and the message and
  exit code of each failure in the exit-code matrix, at both CLI feature
  levels (`sruman/tests/snapshots/`). Every error names a fix: a command, a
  file and key, or what to stop. Colored output goes through one `paint`
  helper, unit-tested in `output`. The `--json` output of every command
  that works offline (`runtimes`, `info`, `list`, `show`, `profiles list`,
  `profiles show`, `status`) is snapshotted too, so a schema change shows
  up as a diff.
- **Docs and code**: every command and `profiles` subcommand has a row in
  USAGE.md and every row is one; every option is mentioned;
  `examples/config.toml` holds every `config.toml` key and works, as shipped
  and with every commented key enabled; `examples/games.toml` shows every
  game key.
- **Exit codes**: 0 to 4 each come from a real invocation in
  `parse_matrix_exit_codes`.
- **Property tests**: build ids order totally, consistently with `Eq` and
  numerically; build ids and runtime refs round-trip; applying a manifest
  diff to the old manifest gives the new one. Dependency order and `clean`'s
  rules (never the current, newest kept, pinned, linked or in-use build) are
  covered by example tests over every case. **Accepted** as is.
- **Crash consistency**: an upgrade cancelled at the start or in the middle
  of each phase (download, verify, extract, commit) leaves `current` on a
  complete build, no half-installed build visible, `.staging` cleared by the
  next lock, and a retry that works. A killed process leaves the same state
  as a cancelled one, since the build is renamed into place in one step.
- **Concurrency**: two commands on the store (the second gets exit 4),
  removing a build a game runs on (exit 4), starting a game that's already
  running (exit 4), shared prefixes (launches coexist; reset and removal
  wait for none and fail).
- **Coverage**: libsruman 92.8% of lines (89.5% of regions), above the
  PLAN's 85%.

Fuzzing, mutation testing and a scheduled real-server canary were planned
but dropped: the property tests cover the parsers, and the real-server test
(`#[ignore]`d) is run by hand before a release (§5). **Accepted.**

## 4. Summary

| Severity | Fixed | Accepted | Follow-up |
| -------- | ----- | -------- | --------- |
| high | 0 | 0 | 0 |
| medium | 2 (archive size cap, decoder memory) | 0 | 0 |
| low | 8 | 5 | 0 |

Not counted above: the docs fix (§3) and the deviations from PLAN budgets,
accepted with their reasons in §2 (install memory, two reads of the
archive, unstripped binaries).

## 5. Running the audit

| Part | Local command | CI |
| ---- | ------------- | -- |
| Gates (§8 of ARCHITECTURE.md) | `cargo fmt --all -- --check && typos && cargo deny check`, then clippy and tests at the three feature levels | `lint.yml`, `test.yml`, … |
| Static rules | `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test -p sruman --test policy` | `lint.yml`, `test.yml` |
| Hostile archives and server | `cargo test -p libsruman --all-features install:: fetch:: sync::` | `test.yml` |
| Property tests | `PROPTEST_CASES=50000 cargo test --release -p libsruman --all-features --test robustness` | `test.yml` (256 cases) |
| Snapshots | `cargo test -p sruman`; review changes with `cargo insta review` or `INSTA_UPDATE=always` | `test.yml` |
| Coverage | `cargo llvm-cov -p libsruman --all-features --fail-under-lines 85` | `test.yml` (report only) |
| Real server | `SRUMAN_REAL_INSTALL=scout cargo test -p libsruman --test real_server -- --ignored` | — |
| Size | `cargo build --release -p sruman [--no-default-features]`, `cargo tree -e normal` | — |
| Startup, install memory, launch | by hand with the release binary (§2); no stable CI timing | — |
