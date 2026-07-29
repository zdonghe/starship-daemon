<#
.SYNOPSIS
    Benchmark starship prompt: plain subprocess, stock IPC, optional gix-native IPC.
.DESCRIPTION
    Runs 200 samples (after 15 warmup) for each config in the target directory.
    Results printed as a table.

    Env vars:
      STARSHIP_BENCH_DIR    - directory to test (default: current dir)
      STARSHIP_DAEMON_PATH  - path to stock starship-daemon.exe (default build, required)
      STARSHIP_DAEMON_PATH_GIX - path to gix-native daemon (optional, adds gix variants)
      STARSHIP_CONFIG       - path to starship.toml
      STARSHIP_MODULE_PATH  - path to starship-daemon.psm1 (default: auto-detect)
#>

$targetDir = if ($env:STARSHIP_BENCH_DIR) { $env:STARSHIP_BENCH_DIR } else { (Get-Location).ProviderPath }
$daemon    = $env:STARSHIP_DAEMON_PATH
$daemonGix = $env:STARSHIP_DAEMON_PATH_GIX
$cfg       = $env:STARSHIP_CONFIG
$results   = @()

# Auto-detect module path if not specified
$modulePath = if ($env:STARSHIP_MODULE_PATH) {
    $env:STARSHIP_MODULE_PATH
} else {
    $found = $null
    foreach ($probe in @(
        "$PSScriptRoot\..\..\starship-daemon.psm1"
        "$env:USERPROFILE\Documents\dotfiles\configs\Powershell\starship-daemon.psm1"
        "$HOME\.config\powershell\starship-daemon.psm1"
    )) {
        if (Test-Path $probe) { $found = $probe; break }
    }
    $found
}

# ---- 1. Plain starship (subprocess) - skipped (hangs on init) ----

$Warmup = 15; $Samples = 200

# ---- 2+: IPC variants (require module + daemon) ----
if (-not $modulePath -or -not $daemon) {
    Write-Warning "STARSHIP_DAEMON_PATH or module not found; skipping IPC benchmarks"
    exit
}

$variants = @(
    @{label="stock (no-cache)"; cache=0; exe=$daemon},
    @{label="stock + cache";    cache=1; exe=$daemon}
)
if ($daemonGix) {
    $variants += @{label="gix (no-cache)"; cache=0; exe=$daemonGix}
    $variants += @{label="gix + cache";    cache=1; exe=$daemonGix}
}

foreach ($v in $variants) {
    Write-Host "=== $($v.label) ===" -ForegroundColor Cyan

    Get-Process "*starship-daemon*" -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500

    $env:STARSHIP_CONFIG = $cfg
    $env:STARSHIP_DAEMON_PATH = $v.exe
    $env:STARSHIP_DAEMON_CACHE = "$($v.cache)"
    Set-Location $targetDir

    Remove-Module starship-daemon -ErrorAction SilentlyContinue
    Import-Module $modulePath -DisableNameChecking -Force

    function global:prompt {
        $lastExit = $LASTEXITCODE
        if ($lastExit -eq $null) { $lastExit = 0 }
        $result = Get-StarshipPrompt -ExitCode $lastExit -Keymap '' -Width $Host.UI.RawUI.WindowSize.Width
        if ($result) { $result } else { "PS> " }
    }

    1..$Warmup | ForEach-Object { prompt | Out-Null }
    $nullCount = 0
    $times = 1..$Samples | ForEach-Object {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $r = prompt
        $sw.Stop()
        if ([string]::IsNullOrEmpty($r)) { $nullCount++ }
        $sw.Elapsed.TotalMilliseconds
    }
    $s = $times | Sort-Object
    Write-Host ("  {0,-18} mean={1,7:N2} median={2,7:N2} p95={3,7:N2} max={4,7:N2} nulls={5}" -f $v.label, `
        ($times | Measure-Object -Average).Average, $s[$s.Count / 2], $s[[int]($s.Count * 0.95)], $s[-1], $nullCount)
    $results += [PSCustomObject]@{ Config=$v.label; Mean="{0:N2}"-f($times|Measure-Object -Average).Average; Median="{0:N2}"-f$s[$s.Count/2]; P95="{0:N2}"-f$s[[int]($s.Count*0.95)]; Max="{0:N2}"-f$s[-1] }
}

Write-Host "`n=== Results ===" -ForegroundColor Green
$results | Format-Table -AutoSize
