# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. Intended as a drop-in replacement/improvement for Starship.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and sends rendered prompts over `\\.\pipe\starship-daemon`. The client module handles auto-start, pipe communication, and error-code bridging.

## Setup

Add this to your `$PROFILE`:

```powershell
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon.exe"
Import-Module "C:\path\to\starship-daemon.psm1" -DisableNameChecking

# Wrapper for error-code support
$oldPrompt = $Function:prompt
function prompt {
    $global:PromptLastCmdOk = $?
    $global:PromptLastExitCode = $global:LASTEXITCODE
    & $oldPrompt
}
```

The module auto-starts the daemon and replaces `prompt`.

## Config

The daemon reads `$env:STARSHIP_CONFIG` on every request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
```

Hot reloading is supported.

## Caching

The daemon caches prompts by (cwd, exit code, terminal width, git ref mtimes, config mtime). 

Disable caching: `$env:STARSHIP_DAEMON_CACHE = 0`

## Performance

200 samples after 15 warmup rounds, measured from PowerShell client across three directories:
a non-git desktop folder, the [starship](https://github.com/starship/starship) repo and the [Linux kernel](https://github.com/torvalds/linux) repo

| Config | Desktop (non-git) | starship repo (git) | Linux kernel (git) |
|--------|-------------------|---------------------|--------------------|
| `starship prompt` subprocess | 31.25 ms | 171.23 ms | 724.85 ms |
| IPC + stock (no cache) | 0.67 ms | 45.96 ms | 515.24 ms |
| IPC + gix-native (no cache) | 0.79 ms | 11.71 ms | 516.16 ms |
| IPC + gix-native + daemon cache | **0.28 ms** | **0.30 ms** | **0.45 ms** |


## Build

### Default (stock starship)

```
cargo build --release
```

The daemon uses the official `starship` crate from crates.io.

### Gix-native fork (optional)

For faster git status, uncomment two blocks in `Cargo.toml` and build.
Uncomment the `gix` dev-dep in `[dev-dependencies]` and the entire `[patch.crates-io]` block below `[[bin]]`.

```
cargo build --release
```


## Benchmarking

Run `perf/scripts/bench-all.ps1` to reproduce results:

```powershell
# Optional: directory to test (defaults to current)
$env:STARSHIP_BENCH_DIR = "C:\path\to\repo"

# Required for IPC tests
$env:STARSHIP_DAEMON_PATH = "C:\path\to\starship-daemon.exe"
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"

# Optional: gix-native daemon for comparison
$env:STARSHIP_DAEMON_PATH_GIX = "C:\path\to\starship-daemon-gix.exe"

perf/scripts/bench-all.ps1
```

## Future

If for some reason the caching is not sufficient, could try and make "module-specific" caching. For example, the time cache is invalid after 1 minute, and as a result, the entire prompt, including `git` operations, get recomputed. This is wasted work.
