# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. Intended as a drop-in replacement for Starship.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and serves rendered prompts over a named pipe.

## Performance

| Scenario | Daemon | `starship prompt` subprocess |
|---------|:------:|:---------------------------:|
| Any prompt (warm gix) | **~0.2ms** | **~100ms** |
| Cold first prompt | **~50ms** | **~100ms** |

The daemon always renders fresh — no cached stale prompts. Git status is always current.

## Setup

```powershell
cd ~/Documents/code/starship-daemon
cargo build --release
```

Then add to your PowerShell profile:

```powershell
Import-Module "$env:USERPROFILE\Documents\code\starship-daemon\starship-daemon.psm1" -DisableNameChecking
```

This auto-starts the daemon and replaces your prompt function. See `starship-daemon.psm1` for the complete client.

## Dependencies

Only `starship = "1"` — pure `std` + starship. To update:

```powershell
cargo update starship
cargo build --release
```

If it compiles, it works. Starship's 1.x API is stable.

## Config

The daemon reads `$env:STARSHIP_CONFIG` on each request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
# Next prompt picks it up
```

## How it avoids stale git status

Starship's `get_static_repo_status()` in `git_status.rs` caches git status per directory in a process-global `static`. Within a long-lived process (the daemon), this cache persists across prompts — creating a file and immediately prompting would return the old status without it.

The daemon works around this by passing a **unique subdirectory** as `current_dir` on every request (e.g., `cwd/.starship_bust/<pid>`), while keeping the real cwd as `logical_dir` for display. Since the cache key is `current_dir`, each call gets a unique key and performs a fresh git scan. The overhead is one `mkdir` + one `rmdir` per prompt (~0.01ms).

## Known issues

- **gix cache**: Starship's internal gix repository handle caches file stat results. New files created AFTER the first prompt of a daemon session are detected (the bust approach forces a fresh scan), but the scan reads the index which was opened when the daemon started. In practice this works because the index is re-read from disk on each scan. If you observe stale status, restart the daemon.
