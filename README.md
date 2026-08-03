# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. Intended as a drop-in replacement/improvement for Starship.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and sends rendered prompts over `\\.\pipe\starship-daemon`. The client module handles auto-start, pipe communication, and error-code bridging.

## Setup

Add this to your `$PROFILE`:

```powershell
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon.exe"
Import-Module "C:\path\to\starship-daemon.psm1" -DisableNameChecking
```

The module auto-starts the daemon and replaces the `prompt` function. It also preserves `$?` and `$LASTEXITCODE` across prompt rendering, so no wrapper is needed.

To run code before every prompt (like official starship's `Invoke-Starship-PreCommand` hook), define a global function with that name:

```powershell
function Invoke-Starship-PreCommand {
    # Reset cursor shape and write terminal OSC sequences
    Write-Host -NoNewLine "`e[5 q"
    $host.ui.Write("$esc]9;12$bel")
}
```

## Config

The daemon reads `$env:STARSHIP_CONFIG` on every request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
```

Hot reloading is supported.

## Caching

The daemon caches prompts by (cwd, keymap, terminal width, config mtime, watcher generation). The exit code is decoupled from the cache: only the `status` and `character` modules read it, so they are re-rendered live on an exit-code change while the rest of the prompt reuses cached segments.

The watcher tracks the entire directory for changes, which tells us when to invalidate the cache and re-render/re-compute something, such as `git status`.

Disable caching: `$env:STARSHIP_DAEMON_CACHE = 0`

In a stock build, starship exposes no segment API, so the whole rendered prompt is cached instead.

## Performance

200 samples after 15 warmup rounds, measured from PowerShell client across three directories:
a non-git desktop folder, the [starship](https://github.com/starship/starship) repo, and the [Linux kernel](https://github.com/torvalds/linux) repo.

| Config | Desktop (non-git) | starship repo (git) | Linux kernel (git) |
|--------|-------------------|---------------------|--------------------|
| `starship prompt` subprocess | 23.01 ms | 178.51 ms | 700.04 ms |
| IPC + stock (no cache) | 0.76 ms | 50.61 ms | 506.20 ms |
| IPC + gix-native (no cache) | 0.70 ms | 19.73 ms | 508.38 ms |
| IPC + gix-native + daemon cache | **0.20 ms** | **0.14 ms** | **0.21 ms** |


## Build

```
cargo build --release
```

The daemon auto-detects at build time (via `build.rs`) which starship it's compiled against:

- **Default (stock starship)**: the `starship` crate resolves from crates.io. The daemon compiles with the stock render API.
- **Gix-native fork (optional)**: a `[patch.crates-io]` block in `Cargo.toml` pointing at my personal starship fork (`github.com/zdonghe/starship`). When that patch is active, `build.rs` detects it from `Cargo.lock` and enables the fork's segment-level cache API.


## Benchmarking

Run `benches/bench-all.ps1` to reproduce results:

```powershell
# Optional: directory to test (defaults to current)
$env:STARSHIP_BENCH_DIR = "C:\path\to\repo"

# Required for IPC tests
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon.exe"
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"

# Optional: gix-native daemon for comparison
$env:STARSHIP_DAEMON_PATH_GIX = "C:\path\to\starship-daemon-gix.exe"

benches/bench-all.ps1
```
