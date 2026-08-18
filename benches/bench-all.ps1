<#
.SYNOPSIS
    Benchmark the starship daemon over IPC via global:prompt.
.DESCRIPTION
    For each variant it spawns the daemon on an isolated named pipe, waits for the pipe
    to accept connections, imports the module fresh, warms up 15 rounds, then tries 200
    calls through the module's global:prompt.

    Env vars:
      STARSHIP_BENCH_DIR    - directory to test (default: this script's directory)
      STARSHIP_DAEMON_PATH  - path to stock starship-daemon.exe (default build, required for IPC)
      STARSHIP_DAEMON_PATH_FORK - path to fork-native daemon (optional, adds fork variants)
      STARSHIP_CONFIG       - path to starship.toml

    Run this from a dedicated pwsh session: it loads/removes the module, which replaces the
    session's global:prompt, and it restores env vars + cwd afterwards but not the module.
    Requires PowerShell 7.4+.

    Parameters:
      -SkipSubprocess  skip the `starship prompt` subprocess baseline row
#>

param([switch]$SkipSubprocess)

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if ($PSVersionTable.PSVersion -lt [version]'7.4') {
    throw "bench-all.ps1 requires PowerShell 7.4+ (Start-Process -Environment)"
}

$origEnv = @{}
foreach ($n in 'STARSHIP_DAEMON_PIPE','STARSHIP_DAEMON_PATH','STARSHIP_DAEMON_CACHE','STARSHIP_CONFIG','STARSHIP_SHELL','VIRTUAL_ENV_DISABLE_PROMPT') {
    $origEnv[$n] = [Environment]::GetEnvironmentVariable($n)
}
$origLoc = Get-Location

$targetDir = if ($env:STARSHIP_BENCH_DIR) {
    $env:STARSHIP_BENCH_DIR
} else {
    $PSScriptRoot
}
$daemon = $env:STARSHIP_DAEMON_PATH
$daemonFork = $env:STARSHIP_DAEMON_PATH_FORK
$cfg = $env:STARSHIP_CONFIG
$modulePath = "$PSScriptRoot\..\starship-daemon.psm1"

$Warmup = 15; $Samples = 200
$script:spawned = @()
$allResults = @()

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

function Wait-DaemonReady {
    param([string]$PipeName, [int]$TimeoutMs = 15000)
    $deadline = [Environment]::TickCount + $TimeoutMs
    while ([Environment]::TickCount -lt $deadline) {
        $p = $null
        try {
            $p = [System.IO.Pipes.NamedPipeClientStream]::new('.', $PipeName)
            $p.Connect(100)
            return $true
        } catch {
            Start-Sleep -Milliseconds 25
        } finally {
            if ($null -ne $p) {
                $p.Dispose()
            }
        }
    }
    return $false
}

function Stop-TestDaemons {
    foreach ($id in $script:spawned) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
    $script:spawned = @()
    Start-Sleep -Milliseconds 200
}

function Measure-Prompt {
    param(
        [Parameter(Mandatory)][string]     $Label,
        [Parameter(Mandatory)][scriptblock]$Action,       
        [string]                           $NullMarker,   
        [scriptblock]                      $PostWarmup = {},
        [scriptblock]                      $PostSamples = {}
    )
    1..$Warmup | ForEach-Object { & $Action | Out-Null }
    if ($PostWarmup) {
        & $PostWarmup
    }
    $nulls = 0
    $sw = [System.Diagnostics.Stopwatch]::new()
    $times = 1..$Samples | ForEach-Object {
        $sw.Restart()
        $r = & $Action
        $sw.Stop()
        if ($NullMarker) {
            if ([string]$r -eq $NullMarker) {
                $nulls++
            }
        } elseif (-not $r) {
            $nulls++
        }
        $sw.Elapsed.TotalMilliseconds
    }
    if ($PostSamples) {
        & $PostSamples
    }
    $s = $times | Sort-Object
    $mean = ($times | Measure-Object -Average).Average
    $median = ($s[[int]($s.Count / 2) - 1] + $s[[int]($s.Count / 2)]) / 2
    $p95 = $s[[int]($s.Count * 0.95)]
    Write-Host ("  {0,-18} mean={1,7:N2} median={2,7:N2} p95={3,7:N2} max={4,7:N2} nulls={5}" -f $Label, $mean, $median, $p95, $s[-1], $nulls)
    [PSCustomObject]@{
        Config = $Label
        Mean = "{0:N2}" -f $mean
        Median = "{0:N2}" -f $median
        P95 = "{0:N2}" -f $p95
        Max = "{0:N2}" -f $s[-1]
        Nulls = $nulls
    }
}

# ---------------------------------------------------------------------------
# Prepare for testing
# ---------------------------------------------------------------------------

$configToUse = $cfg
if ($daemon -or $daemonFork) {
    if (-not $configToUse) {
        $configToUse = Join-Path $env:USERPROFILE '.config\starship.toml'
    }
    if (-not (Test-Path -LiteralPath $configToUse -PathType Leaf)) {
        throw "no usable STARSHIP_CONFIG ('$configToUse') - daemon would exit(1) ConfigNotFound"
    }
    $configToUse = (Resolve-Path -LiteralPath $configToUse).ProviderPath
}

$resolvedDaemon = $null
$resolvedDaemonFork = $null
if ($daemon) {
    if (Test-Path -LiteralPath $daemon -PathType Leaf) {
        $resolvedDaemon = (Resolve-Path -LiteralPath $daemon).ProviderPath
    } else {
        Write-Warning "Could not resolve STARSHIP_DAEMON_PATH '$daemon'; skipping IPC benchmarks"
    }
}
if ($daemonFork) {
    if (Test-Path -LiteralPath $daemonFork -PathType Leaf) {
        $resolvedDaemonFork = (Resolve-Path -LiteralPath $daemonFork).ProviderPath
    } else {
        Write-Warning "Could not resolve STARSHIP_DAEMON_PATH_FORK '$daemonFork'; skipping fork variants"
    }
}

$variants = @()
if ($resolvedDaemon) {
    $variants += @{ label = "stock (no-cache)"; cache = 0; exe = $resolvedDaemon }
    $variants += @{ label = "stock + cache";    cache = 1; exe = $resolvedDaemon }
}
if ($resolvedDaemonFork) {
    $variants += @{ label = "fork (no-cache)"; cache = 0; exe = $resolvedDaemonFork }
    $variants += @{ label = "fork + cache";    cache = 1; exe = $resolvedDaemonFork }
}

# ---------------------------------------------------------------------------
# Benchmarks
# ---------------------------------------------------------------------------

function Invoke-SubprocessBaseline {
    $exe = (Get-Command starship -CommandType Application -ErrorAction SilentlyContinue).Source
    if (-not $exe) {
        Write-Warning "starship not found on PATH; skipping subprocess baseline row"
        return
    }
    Write-Host "=== starship prompt (subprocess baseline) ===" -ForegroundColor Cyan

    if ($configToUse) {
        $env:STARSHIP_CONFIG = $configToUse
    }
    $spArgs = @('prompt', '--path', $targetDir, '--status', '0', '--terminal-width', '120')

    Measure-Prompt -Label 'starship prompt' -Action { & $exe @spArgs 2>$null }
}

function Invoke-IPCBenchmarks {
    $rows = @()
    for ($vi = 0; $vi -lt $variants.Count; $vi++) {
        $v = $variants[$vi]
        Write-Host "=== $($v.label) ===" -ForegroundColor Cyan

        $pipeName = "starship-daemon-bench-{0}-{1}" -f $vi, ([guid]::NewGuid().ToString('N').Substring(0, 8))

        $proc = Start-Process -FilePath $v.exe -WindowStyle Hidden -PassThru -Environment @{
            STARSHIP_DAEMON_PIPE = $pipeName
            STARSHIP_CONFIG = $configToUse
        }
        $script:spawned += $proc.Id

        if (-not (Wait-DaemonReady -PipeName $pipeName)) {
            throw "FAIL: daemon (pid $($proc.Id)) never became ready on pipe $pipeName"
        }

        $env:STARSHIP_DAEMON_PIPE = $pipeName
        $env:STARSHIP_CONFIG = $configToUse
        $env:STARSHIP_DAEMON_PATH = ''
        $env:STARSHIP_DAEMON_CACHE = "$($v.cache)"
        Set-Location $targetDir

        Remove-Module starship-daemon -ErrorAction SilentlyContinue
        $mod = Import-Module $modulePath -DisableNameChecking -Force -PassThru

        if (& $mod { $script:DaemonDown }) {
            throw "FAIL: latched after import ($($v.label))"
        }

        $probe = & $mod { Get-StarshipPrompt -ExitCode 0 -Keymap '' -Width 80 }
        if (-not $probe) {
            throw "FAIL: Get-StarshipPrompt returned null on first call ($($v.label))"
        }

        $fallback = "PS $((Get-Location).ProviderPath)> "
        $checkWarmup = { if (& $mod { $script:DaemonDown }) {
                throw "FAIL: latched during warmup ($($v.label))"
            } }
        $checkSamples = { if (& $mod { $script:DaemonDown }) {
                throw "FAIL: daemon-down latch was thrown during samples ($($v.label))"
            } }

        $rows += Measure-Prompt -Label $v.label `
            -Action { prompt } `
            -NullMarker $fallback `
            -PostWarmup  $checkWarmup `
            -PostSamples $checkSamples

        Stop-TestDaemons
    }
    $rows
}

# ---------------------------------------------------------------------------
# Run + Cleanup
# ---------------------------------------------------------------------------

try {
    if (-not $SkipSubprocess) {
        $subprocessRow = Invoke-SubprocessBaseline
        if ($subprocessRow) {
            $allResults += $subprocessRow
        }
    }

    if ($variants.Count) {
        $allResults += Invoke-IPCBenchmarks
    } else {
        Write-Warning "no resolvable daemon (STARSHIP_DAEMON_PATH/_FORK); skipping IPC benchmarks"
    }
} finally {
    Stop-TestDaemons
    Remove-Module starship-daemon -ErrorAction SilentlyContinue
    foreach ($n in $origEnv.Keys) {
        if ($null -eq $origEnv[$n]) {
            Remove-Item "Env:$n" -ErrorAction SilentlyContinue
        } else {
            Set-Item "Env:$n" $origEnv[$n]
        }
    }
    Set-Location $origLoc
}

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Green
$allResults | Format-Table -AutoSize
