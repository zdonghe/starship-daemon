<#
.SYNOPSIS
    Benchmark the starship daemon over IPC via the module's real global:prompt.
.DESCRIPTION
    For each variant it spawns the daemon on an isolated named pipe, waits for the pipe
    to accept connections, imports the module fresh, warms up 15 rounds, then times 200
    calls through the module's real global:prompt.

    Any variant that ends with the module in a daemon-down latch is a HARD FAILURE and
    aborts the run - fallback numbers are never reported.

    Also times the stock `starship prompt` subprocess on the same directory as a baseline
    row (skipped when starship is not on PATH). The subprocess row runs standalone even
    when STARSHIP_DAEMON_PATH is unset; the IPC variants are skipped with a warning.
    Subprocess baseline is deterministic: status=0, terminal-width=120, default keymap
    (viins), zero cmd-duration and jobs.

    Env vars:
      STARSHIP_BENCH_DIR    - directory to test (default: current dir)
      STARSHIP_DAEMON_PATH  - path to stock starship-daemon.exe (default build, required for IPC)
      STARSHIP_DAEMON_PATH_GIX - path to gix-native daemon (optional, adds gix variants)
      STARSHIP_CONFIG       - path to starship.toml

    IPC rows measure the full module global:prompt (Get-History exit-code, $global:error[0],
    PSReadLine keymap, terminal-width probe + the pipe round-trip). The subprocess row measures
    only cold `starship prompt` process cost - the two are not a strict render-speed comparison.

    Run this from a dedicated pwsh session: it loads/removes the module, which replaces the
    session's global:prompt, and it restores env vars + cwd afterwards but not the module.
    Requires PowerShell 7+ (Start-Process -Environment).
#>

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw "bench-all.ps1 requires PowerShell 7+ (Start-Process -Environment)"
}

$origEnv = @{}
foreach ($n in 'STARSHIP_DAEMON_PIPE','STARSHIP_DAEMON_PATH','STARSHIP_DAEMON_CACHE','STARSHIP_DAEMON_PATH_GIX','STARSHIP_CONFIG','STARSHIP_SHELL','VIRTUAL_ENV_DISABLE_PROMPT')
{
    $origEnv[$n] = [Environment]::GetEnvironmentVariable($n)
}
$origLoc = Get-Location

$targetDir = if ($env:STARSHIP_BENCH_DIR) { $env:STARSHIP_BENCH_DIR } else { (Get-Location).ProviderPath }
$daemon    = $env:STARSHIP_DAEMON_PATH
$daemonGix = $env:STARSHIP_DAEMON_PATH_GIX
$cfg       = $env:STARSHIP_CONFIG
$modulePath = "$PSScriptRoot\..\starship-daemon.psm1"

$Warmup = 15; $Samples = 200
$testStart = Get-Date
$allResults = @()

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
            if ($null -ne $p) { $p.Dispose() }
        }
    }
    return $false
}

$spawned = @()

function Stop-TestDaemons
{
    foreach ($id in $spawned)
    {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
    $sweepPaths = @($resolvedDaemon) + @($resolvedDaemonGix | Where-Object { $_ })
    Get-CimInstance Win32_Process -Filter "Name LIKE 'starship-daemon%'" -ErrorAction SilentlyContinue |
        Where-Object { $sweepPaths -contains $_.ExecutablePath -and $_.CreationDate -ge $testStart } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 200
}

$configToUse = $cfg
if ($daemon) {
    if ($configToUse) {
        if (-not (Test-Path -LiteralPath $configToUse -PathType Leaf)) {
            Write-Error "STARSHIP_CONFIG '$configToUse' is not a file - daemon would exit(1) ConfigNotFound"
        }
        $configToUse = (Resolve-Path -LiteralPath $configToUse).ProviderPath
    } else {
        $defaultConfig = Join-Path $env:USERPROFILE '.config\starship.toml'
        if (Test-Path -LiteralPath $defaultConfig -PathType Leaf) {
            $configToUse = $defaultConfig
        } else {
            Write-Error "STARSHIP_CONFIG unset and no default at '$defaultConfig' - daemon would exit(1) ConfigNotFound"
        }
    }
}

$resolvedDaemon = $null
$resolvedDaemonGix = $null
if ($daemon) {
    if (Test-Path -LiteralPath $daemon -PathType Leaf) {
        $resolvedDaemon = (Resolve-Path -LiteralPath $daemon).ProviderPath
    } else {
        Write-Warning "Could not resolve STARSHIP_DAEMON_PATH '$daemon'; skipping IPC benchmarks"
    }
    if ($daemonGix) {
        if (Test-Path -LiteralPath $daemonGix -PathType Leaf) {
            $resolvedDaemonGix = (Resolve-Path -LiteralPath $daemonGix).ProviderPath
        } else {
            Write-Warning "Could not resolve STARSHIP_DAEMON_PATH_GIX '$daemonGix'; skipping gix variants"
        }
    }
}

$variants = @()
if ($resolvedDaemon) {
    $variants += @{ label="stock (no-cache)"; cache=0; exe=$resolvedDaemon }
    $variants += @{ label="stock + cache";    cache=1; exe=$resolvedDaemon }
}
if ($resolvedDaemonGix) {
    $variants += @{ label="gix (no-cache)"; cache=0; exe=$resolvedDaemonGix }
    $variants += @{ label="gix + cache";    cache=1; exe=$resolvedDaemonGix }
}

try
{
    $starshipExe = (Get-Command starship -CommandType Application -ErrorAction SilentlyContinue).Source
    if (-not $starshipExe) {
        Write-Warning "starship not found on PATH; skipping subprocess baseline row"
    } else {
        Write-Host "=== starship prompt (subprocess baseline) ===" -ForegroundColor Cyan

        if ($configToUse) { $env:STARSHIP_CONFIG = $configToUse }
        $starshipArgs = @('prompt', '--path', $targetDir, '--status', '0', '--terminal-width', '120')

        1..$Warmup | ForEach-Object { & $starshipExe @starshipArgs 2>$null | Out-Null }

        $nulls = 0
        $times = 1..$Samples | ForEach-Object {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $r = & $starshipExe @starshipArgs 2>$null
            $sw.Stop()
            if (-not $r) { $nulls++ }
            $sw.Elapsed.TotalMilliseconds
        }

        $s = $times | Sort-Object
        $med = [int]($s.Count / 2)
        $median = ($s[$med - 1] + $s[$med]) / 2
        Write-Host ("  {0,-18} mean={1,7:N2} median={2,7:N2} p95={3,7:N2} max={4,7:N2} nulls={5}" -f 'starship prompt', `
            ($times | Measure-Object -Average).Average, $median, $s[[int]($s.Count * 0.95)], $s[-1], $nulls)
        $allResults += [PSCustomObject]@{
            Config = 'starship prompt'
            Mean   = "{0:N2}" -f ($times | Measure-Object -Average).Average
            Median = "{0:N2}" -f $median
            P95    = "{0:N2}" -f $s[[int]($s.Count * 0.95)]
            Max    = "{0:N2}" -f $s[-1]
            Nulls  = $nulls
        }
    }

    if ($variants.Count)
    {
        for ($vi = 0; $vi -lt $variants.Count; $vi++)
        {
            $v = $variants[$vi]
            Write-Host "=== $($v.label) ===" -ForegroundColor Cyan

            $pipeName = "starship-daemon-bench-{0}-{1}" -f $vi, ([guid]::NewGuid().ToString('N').Substring(0, 8))

            $proc = Start-Process -FilePath $v.exe -WindowStyle Hidden -PassThru -Environment @{
                STARSHIP_DAEMON_PIPE  = $pipeName
                STARSHIP_CONFIG       = $configToUse
            }
            $spawned += $proc.Id

            if (-not (Wait-DaemonReady -PipeName $pipeName))
            {
                throw "FAIL: daemon (pid $($proc.Id)) never became ready on pipe $pipeName"
            }

            $env:STARSHIP_DAEMON_PIPE  = $pipeName
            $env:STARSHIP_CONFIG       = $configToUse
            $env:STARSHIP_DAEMON_PATH  = ''
            $env:STARSHIP_DAEMON_CACHE = "$($v.cache)"
            Set-Location $targetDir

            Remove-Module starship-daemon -ErrorAction SilentlyContinue
            $mod = Import-Module $modulePath -DisableNameChecking -Force -PassThru

            if (& $mod { $script:DaemonDown })
            {
                throw "FAIL: latched after import ($($v.label))"
            }

            $probe = & $mod { Get-StarshipPrompt -ExitCode 0 -Keymap '' -Width 80 }
            if (-not $probe)
            {
                throw "FAIL: Get-StarshipPrompt returned null on first call ($($v.label))"
            }

            1..$Warmup | ForEach-Object { prompt | Out-Null }

            if (& $mod { $script:DaemonDown })
            {
                throw "FAIL: latched during warmup ($($v.label))"
            }

            $fallback = "PS $((Get-Location).ProviderPath)> "
            $nulls = 0
            $times = 1..$Samples | ForEach-Object {
                $sw = [System.Diagnostics.Stopwatch]::StartNew()
                $r = prompt
                $sw.Stop()
                if ([string]$r -eq $fallback) { $nulls++ }
                $sw.Elapsed.TotalMilliseconds
            }

            if (& $mod { $script:DaemonDown })
            {
                throw "FAIL: daemon-down latch was thrown during samples ($($v.label))"
            }

            $s = $times | Sort-Object
            $med = [int]($s.Count / 2)
            $median = ($s[$med - 1] + $s[$med]) / 2
            Write-Host ("  {0,-18} mean={1,7:N2} median={2,7:N2} p95={3,7:N2} max={4,7:N2} nulls={5}" -f $v.label, `
                ($times | Measure-Object -Average).Average, $median, $s[[int]($s.Count * 0.95)], $s[-1], $nulls)
            $allResults += [PSCustomObject]@{
                Config = $v.label
                Mean   = "{0:N2}" -f ($times | Measure-Object -Average).Average
                Median = "{0:N2}" -f $median
                P95    = "{0:N2}" -f $s[[int]($s.Count * 0.95)]
                Max    = "{0:N2}" -f $s[-1]
                Nulls  = $nulls
            }

            Stop-TestDaemons
            $spawned = @()
        }
    }
    else
    {
        Write-Warning "no resolvable daemon (STARSHIP_DAEMON_PATH/_GIX); skipping IPC benchmarks"
    }
}
finally
{
    Stop-TestDaemons
    Remove-Module starship-daemon -ErrorAction SilentlyContinue
    foreach ($n in $origEnv.Keys)
    {
        if ($null -eq $origEnv[$n]) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
        else { Set-Item "Env:$n" $origEnv[$n] }
    }
    Set-Location $origLoc
}

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Green
$allResults | Format-Table -AutoSize
