# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. Intended as a drop-in replacement for Starship.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and serves rendered prompts over a named pipe.

## Performance

todo: actual perf numbers, starship lonesome, starship-daemon, starship-daemon + cache. test in normal repos, git repos, battery perf.

## Setup

```powershell
$env:STARSHIP_DAEMON_PATH="c:\path\to\daemon"
Import-Module "c:\path\to\starship-daemon.psm1" -DisableNameChecking

# for error code support, add this to prompt function
function prompt {
    $script:PromptLastCmdOk = $?
    $script:PromptLastExitCode = $global:LASTEXITCODE
}
```

This auto-starts the daemon and replaces your prompt function. See `starship-daemon.psm1` for the complete client.

## Config

The daemon reads `$env:STARSHIP_CONFIG` on each request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
```

Hot reloading is supported.

## Caching
Caching dramatically reduces prompt latency and reduces redundant work. However, it might result in stale/inaccurate prompts.

Caching can be disabled by setting `$env:STARSHIP_DAEMON_CACHE=0`.

## Build
```
cargo build --release
```


