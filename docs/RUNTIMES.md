# Runtimes

What sruman installs, where it gets it, and how that's checked.

## The runtimes

| Name         | Server dir | Archive                              | Base           | Runs as                                   |
| ------------ | ---------- | ------------------------------------ | -------------- | ----------------------------------------- |
| `scout`      | `steamrt1` | `SteamLinuxRuntime.tar.xz`           | Ubuntu 12.04   | inside a `soldier` container (needs it)   |
| `soldier`    | `steamrt2` | `SteamLinuxRuntime_soldier.tar.xz`   | Debian 10      | container (pressure-vessel)               |
| `sniper`     | `steamrt3` | `SteamLinuxRuntime_sniper.tar.xz`    | Debian 11      | container (pressure-vessel)               |
| `steamrt4`   | `steamrt4` | `SteamLinuxRuntime_4.tar.xz` (+ `-arm64`) | Debian 13 | container (pressure-vessel)               |
| `steamrt3c`  | `steamrt3c` | `steam-runtime-steamrt-amd64.tar.xz` (+ `-arm64`) | Debian 11 | Steam's 64-bit client (not games) |

`steamrt4` is the latest and the one to use for native games and new
profiles. The older runtimes stay for games built against them and for
Protons whose `toolmanifest.vdf` asks for them. `steamrt3c` is the runtime
Steam's own 64-bit client runs in; sruman installs it so `sruman steam link`
can give Steam its client runtime too (see [Steam](USAGE.md#steam)). Its
build ids start with `3c` (`3c.0.20261103.268067`).

## Where builds come from

Everything comes from `https://repo.steampowered.com/<server dir>/images/`:

- `update-timestamp.txt` changes whenever anything is published, so
  `sruman sync` costs one small request per server directory when nothing
  changed.
- `latest-public-stable.txt`, `latest-public-beta.txt` and
  `latest-container-runtime-depot.txt` (the `steam` channel, falling back to
  `latest-steam-client-general-availability.txt`) each name a build.
- `<build>/SHA256SUMS` lists every file of a build. sruman takes the
  archive's sum from it, and finds the package list and os-release by
  Valve's naming:
  `com.valvesoftware.SteamRuntime.Platform-<amd64,i386|arm64>-<suite>.manifest.dpkg`
  and `….os-release.txt` (never the `Sdk` ones).

Builds never change once published, so the sync index only fetches builds
it hasn't seen. There are no signatures: integrity is HTTPS plus
`SHA256SUMS`. A `base_url` override must be `https://`, except for a
mirror on `localhost`.

## Channels

`stable` (`latest-public-stable.txt`) is the default. `beta` follows
`latest-public-beta.txt`, which Valve updates first. `steam` is what the
Steam client uses right now (`latest-container-runtime-depot.txt`). Set
`channel` in `config.toml`, or per runtime under `[channels]`; `sruman
builds steamrt4` lists every build on the server, and `steamrt4=<build>`
installs or pins one.

## Why images and not apt

Valve also publishes an apt repository, but it holds the hundreds of
`.deb`s a runtime is *built* from, not the runtime: installing from it would
mean assembling a pressure-vessel container the way Valve's pipeline does,
rather than using the exact image games are tested against. `images/`
already gives what apt would: channel pointers, versioned build directories
(history and downgrades), `SHA256SUMS`, and each build's `manifest.dpkg`.
sruman uses the manifests for apt-style tracking: `info --packages`,
`search --packages`, and the package changes an `upgrade` shows
(`mesa 25.1 → 25.2`). The cost: images are checked by HTTPS and checksum,
while apt is GPG-signed.

## How an install is checked

1. The archive streams into `~/.cache/sruman/pkg/` while its SHA-256 is
   computed, and must match `SHA256SUMS`. A partial download is kept as
   `.part` and resumed with HTTP `Range`.
2. Extraction accepts only regular files, directories and relative
   symlinks whose `..` steps come first and stay inside the build (all of
   Valve's 1,520 symlinks have that form). Anything else fails the install:
   absolute or `..` paths, hardlinks, devices, writing through a symlink.
   Setuid/setgid bits are dropped.
   Unpacking stops past twice the packages' `Installed-Size` (20 times the
   archive when the manifest isn't known, never below 512 MiB) or a million
   entries, so a broken or hostile archive can't fill the disk.
3. The build is renamed into the store in one step; an interrupted install
   leaves only `.staging/`, which the next command clears. `sruman verify
   --full` later checks every file against Valve's own `mtree.txt.gz` and
   `usr-mtree.txt.gz`.

Redirects from HTTPS to plain HTTP are refused, downloads that stall for a
minute fail instead of hanging, and the small files (pointers, sums,
manifests) are capped at 16 MiB.

## Using Steam's or umu's copy

Steam keeps its runtimes in
`~/.local/share/Steam/steamapps/common/SteamLinuxRuntime_{soldier,sniper,4}`
(and `SteamLinuxRuntime` for scout). `sruman import <dir>` copies one into
the store; `sruman import --link <dir>` registers it in place, so Steam keeps
updating it and sruman never deletes its files (`clean` skips linked builds).
A runtime umu downloaded (`~/.local/share/umu/<variant>`) imports the same
way; delete umu's copy afterwards and let `sruman provide-umu` hand the
runtime to umu from then on.

To go the other way, so that Steam runs on sruman's builds instead of
keeping its own copies, use `sruman steam link` rather than `import`: it
imports what's missing and puts links in Steam's folders, including the
client's runtime (`steamrt64/pv-runtime/steam-runtime-steamrt`). Deleting
Steam's copies doesn't work: Steam checks its files at every start and
downloads them again. See [Steam](USAGE.md#steam).

## Troubleshooting

- **`bwrap: setting up uid map: Permission denied`**, or `doctor` reporting
  that user namespaces are off: the container runtimes need unprivileged
  user namespaces. Most distributions enable them; check
  `sysctl kernel.unprivileged_userns_clone` (Debian kernels) and
  `user.max_user_namespaces` (must be above 0). Ubuntu 24.04 and later also
  restrict them through AppArmor
  (`kernel.apparmor_restrict_unprivileged_userns`). Overlay mode
  (`mode = "overlay"`) needs no namespaces at all.
- **`Cannot determine ld.so for i386-linux-gnu`**: a warning on hosts
  without 32-bit libraries; 64-bit games are unaffected.
- **A game can't see a directory**: in container mode with a prefix, only
  the working directory, the prefix and the paths in
  `filesystems_rw`/`filesystems_ro` are visible, not the rest of your
  home.
- **`the runtime store is locked`**: another sruman is installing or
  removing; it's released when that command ends. **`… is in use by a
  running game`**: stop it first; `sruman status` shows which.
- **Anything else**: `sruman doctor`, then the run log (`sruman logs
  <game>`), which holds the exact command and environment. `--debug` on
  `run`/`execute` makes pressure-vessel verbose.

## Manual checks before a release

What the automated tests can't cover, run on a real system:

1. One game per runtime: scout (Hollow Knight) and a sniper-era title.
2. Two profiles sharing a prefix; both start, and `profiles delete --prefix`
   refuses while one runs.
3. `upgrade` while a game runs: the old build stays, `clean` skips it.
4. `import --link` of Steam's sniper, then a game on it.
5. The same native game in container mode with a prefix (saves land in
   `<prefix>/home`) and in overlay mode without one (saves in the real home;
   `LD_DEBUG=libs` shows runtime libraries where they're newer, and the
   host's glibc and Mesa).
6. An SDL `host` override in overlay mode.
7. `provide-umu` for a Proton that needs sniper, then a umu launch through
   lootrs.

## Testing against the real server

The integration tests use a local fake of the server. To check against
Valve's real one (network; downloads about 190 MB for sniper):

```sh
cargo test -p libsruman --test real_server -- --ignored --nocapture
# another runtime, and keep the files to look at:
SRUMAN_REAL_INSTALL=scout SRUMAN_REAL_DIR=/tmp/sruman-real \
  cargo test -p libsruman --test real_server -- --ignored --nocapture
```

It refreshes every runtime, lists sniper's builds, installs the chosen
runtime (with dependencies) and verifies it at content depth. To check the
extractor alone on an archive you already have:

```sh
SRUMAN_REAL_ARCHIVE=/path/SteamLinuxRuntime_sniper.tar.xz \
  cargo test --release -p libsruman --lib -- --ignored real_archive --nocapture
```
