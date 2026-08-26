# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. Intended as a drop-in replacement/improvement for Starship.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and sends rendered prompts over `\\.\pipe\starship-daemon`. The client module handles auto-start, pipe communication, and error-code bridging.

## Install

Requires PowerShell 5.1+ and Windows 10/11 x64.

1. Grab the latest release: the zip from the [releases page](https://github.com/zdonghe/starship-daemon/releases) contains `starship-daemon-stock.exe`, `starship-daemon-fork.exe`, and `starship-daemon.psm1`. (Or build from source - see [Build](#build).)
2. Put the `exe` and `psm1` in a folder of your choice.
3. Add this to your `$PROFILE`:

```powershell
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon.exe"
Import-Module "C:\path\to\starship-daemon.psm1" -DisableNameChecking
```

The module auto-starts the daemon and replaces the `prompt` function.

> Use the `-fork.exe` binary for segment-level caching (fastest, uses the [fork](https://github.com/zdonghe/starship)). A plain `cargo build` produces this variant by default.

## Config

The daemon reads `$env:STARSHIP_CONFIG` on every request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
```

Hot reloading is supported.

## Troubleshooting

Try the following command:

```powershell
Restart-StarshipDaemon
```

This is also the fix if the module keeps falling back to the plain prompt after a daemon crash - while the daemon is down the module fails to the plain prompt, and it stays that way until you run `Start-StarshipDaemon` or `Restart-StarshipDaemon`.

## Uninstall

Remove the `$env:STARSHIP_DAEMON_PATH` line and the `Import-Module` line from your `$PROFILE`, open a new shell, then optionally stop the daemon:

```powershell
Stop-StarshipDaemon
```

## Build

The default feature is `fork`, so a plain build produces the fork-native binary:

```powershell
# Fork build (default)
cargo build --release

# Stock variant (stock starship)
cargo build --release --no-default-features --features stock
```

`starship-daemon --version` prints the compiled variant (`stock` or `fork`).


## Performance

200 samples after 15 warmup rounds, measured from PowerShell client across three directories:
a non-git desktop folder, the [starship](https://github.com/starship/starship) repo, and the [Linux kernel](https://github.com/torvalds/linux) repo.
Reproduce with `benches/bench-all.ps1`.

| Config | Desktop (non-git) | starship repo (git) | Linux kernel (git) |
|--------|-------------------|---------------------|--------------------|
| `starship prompt` subprocess | 32.04 ms | 208.42 ms | 745.90 ms |
| IPC + stock (no cache) | 1.13 ms | 61.94 ms | 510.34 ms |
| IPC + fork-native (no cache) | 0.97 ms | 13.53 ms | 412.96 ms |
| IPC + fork-native + daemon cache | **0.20 ms** | **0.17 ms** | **0.12 ms** |

## Caching

The daemon caches prompts by (cwd, keymap, terminal width, config mtime, watcher generation).

The watcher tracks the entire directory for changes, which tells us when to invalidate the cache and re-render/re-compute something, such as `git status`.

Disable caching: `$env:STARSHIP_DAEMON_CACHE = 0`

In a stock build, starship exposes no segment API, so the whole rendered prompt is cached instead. If the daemon is built with the fork, segment-level caching is used: every module is cached **except** `time`, `character`, `status`, and the `all` placeholder (those re-render live on every prompt). If any other module becomes invalid/outdated, that specific module can be recomputed without recomputing the entire prompt.

## Diagnostics

`Get-StarshipDaemonTimings` sends a one-off profiling request to the running daemon and returns render-path timings plus a per-module breakdown for the current directory as a string:

```powershell
Get-StarshipDaemonTimings [-Cwd <dir>] [-ExitCode 0] [-Keymap emacs]
```

## Environment variables

| Variable | Used by | Meaning |
|----------|---------|---------|
| `STARSHIP_DAEMON_PATH` | psm1 | Path to the daemon binary to auto-start |
| `STARSHIP_DAEMON_PIPE` | daemon + client | Override the named pipe path (default `\\.\pipe\starship-daemon`) |
| `STARSHIP_DAEMON_CACHE` | psm1 | Set to `0` to bypass the daemon's render cache |
| `STARSHIP_CONFIG` | both | starship config file; changes are hot-reloaded |
| `STARSHIP_DAEMON_THROTTLE` | daemon | Set to `1` to keep Windows power throttling active (by default the daemon disables it) |
| `STARSHIP_DAEMON_NO_AUTOSTART` | psm1 | Set to any value to prevent the module from auto-starting the daemon on import |

## Benchmarking

Run `benches/bench-all.ps1` to reproduce results:

```powershell
# Optional: directory to test (defaults to the script's folder)
$env:STARSHIP_BENCH_DIR = "C:\path\to\repo"

# Required for IPC tests
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon-stock.exe"
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"

# Optional: fork-native daemon for comparison
$env:STARSHIP_DAEMON_PATH_FORK = "C:\path\to\starship-daemon-fork.exe"

benches/bench-all.ps1
```
