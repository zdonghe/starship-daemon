# starship-daemon

A persistent daemon that renders your Starship prompt over a Windows named pipe.

## How it works

Instead of calling `starship prompt ...` as a subprocess on every prompt, the daemon keeps Starship loaded in memory and serves rendered prompts over a named pipe.

## Setup

1. **Build the daemon:**

```powershell
cargo build --release
```

2. **Add to your PowerShell profile:**

```powershell
# Auto-start daemon (handles pipe connection internally)
. "$env:USERPROFILE\Documents\code\starship-daemon\starship-daemon.ps1"
Start-StarshipDaemon

# In your prompt() function:
$result = Get-StarshipPrompt -ExitCode $global:LASTEXITCODE -Keymap $keymap -Width $Host.UI.RawUI.WindowSize.Width
if ($result) { return $result }
```

See `starship-daemon.psm1` for the full client implementation.

## Performance

| Scenario | Daemon | `starship prompt` subprocess |
|---------|:------:|:---------------------------:|
| Cache hit (same dir + exit code) | **0.1ms** | — |
| Cache miss (new dir, warm gix) | **~5ms** | — |
| Cold first request | **~50ms** | **~100ms** |
| Every subsequent prompt | **0.1ms** | **~100ms** |

The cache key is `(directory, exit_code, keymap, 5-minute bucket)`. Most prompts are cache hits.

## Dependencies

- `starship = "1"` — accepts any 1.x version
- No serde, no toml_edit, no git-fast — pure `std` + starship

To update starship:

```powershell
cargo update starship
cargo build --release
```

If it compiles, it works. If it breaks, cargo tells you.

## Migrating from direct Starship

You don't need to change your `starship.toml` at all. The daemon passes all properties (status code, keymap, terminal width) to Starship internally. Your existing config renders identically.

The only change is: instead of your shell calling `starship prompt ...`, it calls the daemon's pipe. The output is byte-identical.

## Config

The daemon reads `$env:STARSHIP_CONFIG` on each request. To switch configs:

```powershell
$env:STARSHIP_CONFIG = "C:\path\to\starship.toml"
# Next prompt picks it up
```

The daemon also watches the config file for changes and reloads automatically.

## Files

| File | Role |
|------|------|
| `target/release/starship-daemon.exe` | The daemon binary |
| `starship-daemon.psm1` | PowerShell client module (auto-start + pipe request function) |
| `src/main.rs` | Named pipe server, config watcher, prompt cache |
| `src/prompt/mod.rs` | Render pipeline (delegates to `starship::print::get_prompt()`) |
