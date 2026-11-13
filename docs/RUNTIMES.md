# Runtimes

What sruman installs, where it gets it, and how that's checked.

## The runtimes

| Name         | Server dir | Archive                              | Base           | Runs as                                   |
| ------------ | ---------- | ------------------------------------ | -------------- | ----------------------------------------- |
| `scout`      | `steamrt1` | `SteamLinuxRuntime.tar.xz`           | Ubuntu 12.04   | inside a `soldier` container (needs it)   |
| `scout-ldlp` | `steamrt1` | `steam-runtime.tar.xz`               | Ubuntu 12.04   | no container (`LD_LIBRARY_PATH`)          |
| `soldier`    | `steamrt2` | `SteamLinuxRuntime_soldier.tar.xz`   | Debian 10      | container (pressure-vessel)               |
| `sniper`     | `steamrt3` | `SteamLinuxRuntime_sniper.tar.xz`    | Debian 11      | container (pressure-vessel)               |
| `steamrt4`   | `steamrt4` | `SteamLinuxRuntime_4.tar.xz` (+ `-arm64`) | Debian 13 | container (pressure-vessel)               |

## Where builds come from

Everything comes from `https://repo.steampowered.com/<server dir>/images/`:

- `update-timestamp.txt` changes whenever anything is published, so `-Sy`
  costs one small request per server directory when nothing changed.
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

## How an install is checked

1. The archive streams into `~/.cache/sruman/pkg/` while its SHA-256 is
   computed, and must match `SHA256SUMS`. A partial download is kept as
   `.part` and resumed with HTTP `Range`.
2. Extraction accepts only regular files, directories and relative
   symlinks whose `..` steps come first and stay inside the build (all of
   Valve's 1,520 symlinks have that form). Anything else fails the install:
   absolute or `..` paths, hardlinks, devices, writing through a symlink.
   Setuid/setgid bits are dropped.
3. The build is renamed into the store in one step. `-Qkk` later checks
   every file against Valve's own `mtree.txt.gz` and `usr-mtree.txt.gz`.

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
