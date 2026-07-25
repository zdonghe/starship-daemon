# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe. No subprocess spawn per prompt.

## How it works

- `starship-daemon.exe` runs in the background, loads `starship.toml` once
- PowerShell's `prompt()` function sends a request over `\.\pipe\starship-daemon`
- Daemon renders the prompt via Starship and returns it
- On pipe failure, falls back to `starship prompt ...` automatically

## Config file

The daemon reads `$env:STARSHIP_CONFIG` at startup and checks it on every request. To switch configs on the fly:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
# Next prompt picks it up immediately
```

For a permanent change, edit the path in `Microsoft.PowerShell_profile.ps1`.

## Restarting

Kill and let auto-start handle it:

```powershell
Get-Process starship-daemon | Stop-Process
# Next prompt starts a fresh daemon
```

## Profiles

The profile provides two prompt configs in `configs/starship/`:

| Config | When | Format |
|--------|------|--------|
| `starship.toml` | Default | Native Starship modules (`$git_branch`, `$git_status`) |
| `git.toml` | Git repos (old) | Combined `${custom.git-status}` from git-fast cache |
| `nogit.toml` | Non-git (old) | No git info |

## Dependencies

- Built from `~/Documents/code/starship-daemon/`
- git-fast watcher at `~/Documents/code/git-fast/` (path dependency)
