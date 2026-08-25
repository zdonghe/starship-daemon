$script:DaemonPath = $env:STARSHIP_DAEMON_PATH
$script:LastStarshipConfig = $null
$script:DaemonPipe = $null
$script:RespBuf = $null
$script:DaemonDown = $false

$script:DaemonPipeName = if ($env:STARSHIP_DAEMON_PIPE) {
    $env:STARSHIP_DAEMON_PIPE
} else {
    "starship-daemon"
}
$script:LastBuildKey = $null
$script:LastBuildBuf = $null
if (-not (Test-Path function:Invoke-Starship-PreCommand)) {
    function global:Invoke-Starship-PreCommand {}
}

$script:FrameSrc = @"
using System;
using System.Text;
public static class StarshipFrame {
    static void W32(byte[] b, int o, uint v) { b[o] = (byte)v; b[o + 1] = (byte)(v >> 8); b[o + 2] = (byte)(v >> 16); b[o + 3] = (byte)(v >> 24); }
    static void W16(byte[] b, int o, ushort v) { b[o] = (byte)v; b[o + 1] = (byte)(v >> 8); }
    public static byte[] Build(string cwd, int exitCode, string keymap, int width, string config, byte disableCache, byte kind = 1) {
        byte[] cb = Encoding.UTF8.GetBytes(cwd);
        if (cb.Length > 32768) return null;
        byte[] kb = keymap == null ? new byte[0] : Encoding.UTF8.GetBytes(keymap);
        byte[] cf = config == null ? new byte[0] : Encoding.UTF8.GetBytes(config);
        int bl = 17 + cb.Length + kb.Length + cf.Length;
        byte[] buf = new byte[5 + bl];
        buf[0] = kind;
        W32(buf, 1, (uint)bl);
        int o = 5;
        W32(buf, o, (uint)cb.Length); o += 4;
        Buffer.BlockCopy(cb, 0, buf, o, cb.Length); o += cb.Length;
        W32(buf, o, unchecked((uint)exitCode)); o += 4;
        W16(buf, o, (ushort)kb.Length); o += 2;
        Buffer.BlockCopy(kb, 0, buf, o, kb.Length); o += kb.Length;
        W32(buf, o, (uint)width); o += 4;
        W16(buf, o, (ushort)cf.Length); o += 2;
        Buffer.BlockCopy(cf, 0, buf, o, cf.Length); o += cf.Length;
        buf[o] = disableCache;
        return buf;
    }
    public static string Parse(byte[] b, int read) {
        if (read < 4) return null;
        int len = (int)(b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24));
        if (len <= 0 || len > 65531 || read < 4 + len) return null;
        return Encoding.UTF8.GetString(b, 4, len);
    }
}
"@
$frameHash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($script:FrameSrc))).Replace('-', '')
$frameAsm = Join-Path $PSScriptRoot "StarshipFrame-$frameHash.dll"
try {
    if (Test-Path -LiteralPath $frameAsm) {
        Add-Type -Path $frameAsm
    } else {
        Add-Type -TypeDefinition $script:FrameSrc -OutputAssembly $frameAsm
        Add-Type -Path $frameAsm
    }
} catch {
    Remove-Item -LiteralPath $frameAsm -Force -ErrorAction SilentlyContinue
    if ($null -eq ('StarshipFrame' -as [type])) {
        Add-Type -TypeDefinition $script:FrameSrc
    }
}

function Start-StarshipDaemon {
    $script:DaemonDown = $false
    if ([string]::IsNullOrEmpty($script:DaemonPath)) {
        return
    }
    $procName = [System.IO.Path]::GetFileNameWithoutExtension($script:DaemonPath)
    $mutex = $null
    try {
        $mutex = [System.Threading.Mutex]::new($false, "Local\starship-daemon-launch")
        try {
            $null = $mutex.WaitOne()
        } catch {}
        if ((Get-Process -Name $procName -ErrorAction SilentlyContinue).Count -eq 0) {
            $null = Start-Process -FilePath $script:DaemonPath -WindowStyle Hidden
        }
    } finally {
        if ($null -ne $mutex) {
            try {
                $mutex.ReleaseMutex()
            } catch {}
            $mutex.Dispose()
        }
    }
}

function Get-StarshipRespBuf {
    # Shared by prompt (sync Read) and timings (async ReadAsync). Safe: the runspace serializes
    # calls, and the timings timeout path disposes the pipe, which aborts its pending read.
    if ($null -eq $script:RespBuf) {
        $script:RespBuf = [byte[]]::new(65536)
    }
    # Leading comma prevents the pipeline from unrolling byte[] into Object[],
    # which would make Read() bind to a converted copy instead of this buffer.
    return ,$script:RespBuf
}

function Get-StarshipPrompt {
    param([int]$ExitCode, [string]$Keymap, [int]$Width)

    # Permanent latch: an IO failure stops further attempts for this session.
    # Recover with Start-StarshipDaemon / Restart-StarshipDaemon.
    if ($script:DaemonDown) {
        return $null
    }

    $pipe = $script:DaemonPipe
    if ($null -eq $pipe) {
        $pipeName = $script:DaemonPipeName
        $p = $null
        try {
            $p = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeName)
            # 250ms: the daemon may not be up yet at the first prompt after
            # boot. Long enough to catch it coming up, short enough that a
            # one-shot miss (daemon truly absent) stays tolerable.
            $p.Connect(250)
            $p.ReadMode = [System.IO.Pipes.PipeTransmissionMode]::Message
            $pipe = $p
        } catch {
            if ($null -ne $p) {
                $p.Dispose()
            }
            $script:DaemonDown = $true
            return $null
        }
        $script:DaemonPipe = $pipe
    }

    $cwd = $PWD.ProviderPath

    $currentCfg = $null
    $cfgChanged = $false
    $result = $null
    $ioFailed = $false
    try {
        $currentCfg = $env:STARSHIP_CONFIG
        $cfgChanged = $currentCfg -ne $script:LastStarshipConfig
        $config = if ($cfgChanged) {
            $currentCfg
        } else {
            $null
        }
        $disableCache = 0
        if ($env:STARSHIP_DAEMON_CACHE -eq "0") {
            $disableCache = 1
        }

        $buildKey = "$cwd|$ExitCode|$Keymap|$Width|$config|$disableCache"
        if ($buildKey -ne $script:LastBuildKey) {
            $script:LastBuildKey = $buildKey
            $script:LastBuildBuf = [StarshipFrame]::Build($cwd, $ExitCode, $Keymap, $Width, $config, [byte]$disableCache)
        }
        $request = $script:LastBuildBuf
        if ($null -eq $request) {
            return $null
        }

        # Armed only across actual IO: Build/config errors above must not
        # kill a healthy pipe. Mirrors Get-StarshipDaemonTimings.
        $ioFailed = $true
        $pipe.Write($request, 0, $request.Length)

        $respBuf = Get-StarshipRespBuf
        $task = $pipe.ReadAsync($respBuf, 0, $respBuf.Length)
        if (-not $task.Wait(5000)) {
            throw [System.TimeoutException]::new("starship-daemon read timed out")
        }
        $read = $task.Result
        $result = [StarshipFrame]::Parse($respBuf, $read)
        $ioFailed = $false
    } catch {
        # Never let diagnostics escape this catch: under DebugPreference
        # Stop/Inquire a Write-Debug throw would skip the dispose+latch below.
        if ($DebugPreference -notin 'Stop', 'Inquire') {
            Write-Debug "Get-StarshipPrompt: $_"
        }
    }

    if ($ioFailed) {
        $pipe.Dispose()
        $script:DaemonPipe = $null
        $script:DaemonDown = $true
        return $null
    }

    # Partial/oversized message: framing suspect, but the daemon itself is
    # healthy - discard the pipe WITHOUT latching so the next prompt retries.
    if ($null -eq $result -and -not $pipe.IsMessageComplete) {
        $pipe.Dispose()
        $script:DaemonPipe = $null
    }

    if ($cfgChanged) {
        $script:LastStarshipConfig = $currentCfg
    }
    return $result
}

function Get-StarshipDaemonTimings {
    <#
    .SYNOPSIS
    Requests a timings report from the running starship-daemon.
    .DESCRIPTION
    Sends a type-2 request over the module's existing daemon pipe and returns
    the report as a string (render-path rows plus per-module timings of the
    resolved format). Measures uncached cold/warm renders and the cached hit
    path; the report briefly blocks the daemon, so other terminals' prompts
    queue behind it.
    .PARAMETER Cwd
    Directory to profile. Defaults to the current location.
    .PARAMETER ExitCode
    Simulated last exit code for the report. Default 0.
    .PARAMETER Keymap
    Simulated keymap for the report. Default 'emacs'.
    .EXAMPLE
    Get-StarshipDaemonTimings
    .EXAMPLE
    Get-StarshipDaemonTimings -Cwd C:\repos\demo -ExitCode 1 -Keymap vi
    #>
    param(
        [string]$Cwd = $PWD.ProviderPath,
        [int]$ExitCode = 0,
        [string]$Keymap = "emacs"
    )

    $pipe = $script:DaemonPipe
    if ($null -eq $pipe) {
        $pipe = try {
            $pipeName = $script:DaemonPipeName
            $p = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeName)
            try {
                $p.Connect(2000)
                $p.ReadMode = [System.IO.Pipes.PipeTransmissionMode]::Message
                $p
            } catch {
                $p.Dispose()
                throw
            }
        } catch {
            throw "starship-daemon is not reachable on pipe '$pipeName'. Start it with Start-StarshipDaemon."
        }
        $script:DaemonPipe = $pipe
    }

    $ioFailed = $false
    try {
        if ([string]::IsNullOrEmpty($Cwd)) {
            $Cwd = $PWD.ProviderPath
        }
        $width = 120
        try {
            $width = $Host.UI.RawUI.WindowSize.Width
        } catch {}
        $config = $env:STARSHIP_CONFIG

        $request = [StarshipFrame]::Build($Cwd, $ExitCode, $Keymap, $width, $config, [byte]0, [byte]2)
        if ($null -eq $request) {
            throw "failed to encode timings request"
        }

        $respBuf = Get-StarshipRespBuf
        $ioFailed = $true
        $pipe.Write($request, 0, $request.Length)
        $task = $pipe.ReadAsync($respBuf, 0, $respBuf.Length)
        if (-not $task.Wait(15000)) {
            throw "timings read timed out after 15s"
        }
        $read = $task.Result
        $ioFailed = $false
        $result = [StarshipFrame]::Parse($respBuf, $read)
        if ($null -eq $result) {
            $ioFailed = $true
            throw "invalid timings response from daemon"
        }
        return $result
    } catch {
        if ($ioFailed) {
            $pipe.Dispose()
            $script:DaemonPipe = $null
        }
        throw
    }
}

function Disconnect-StarshipDaemon {
    if ($null -ne $script:DaemonPipe) {
        $script:DaemonPipe.Dispose()
        $script:DaemonPipe = $null
    }
}

function Stop-StarshipDaemon {
    Disconnect-StarshipDaemon
    if ([string]::IsNullOrEmpty($script:DaemonPath)) {
        return
    }
    $procName = [System.IO.Path]::GetFileNameWithoutExtension($script:DaemonPath)
    Get-Process -Name $procName -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    $script:DaemonDown = $false
}

function Restart-StarshipDaemon {
    Stop-StarshipDaemon
    Start-StarshipDaemon
}

$MyInvocation.MyCommand.ScriptBlock.Module.OnRemove = { Disconnect-StarshipDaemon }

$env:VIRTUAL_ENV_DISABLE_PROMPT = 1
$env:STARSHIP_SHELL = if ($PSVersionTable.PSVersion.Major -gt 5) {
    "pwsh"
} else {
    "powershell"
}

# Set STARSHIP_DAEMON_NO_AUTOSTART=1 to import without spawning/checking the
# daemon (used by the Pester suite for hermetic imports).
if (-not $env:STARSHIP_DAEMON_NO_AUTOSTART) {
    Start-StarshipDaemon
}

try {
    Set-PSReadLineOption -ContinuationPrompt "· "
} catch {}

function global:prompt {
    $origDollarQuestion = $global:?
    $origLastExitCode = $global:LASTEXITCODE

    try {
        Invoke-Starship-PreCommand
    } catch {}

    $exitCode = 0
    if (-not $origDollarQuestion) {
        if ($lastCmd = Get-History -Count 1) {
            $lastCmdletError = try {
                $global:error[0].InvocationInfo
            } catch {
                $null
            }
            if ($null -ne $lastCmdletError -and $lastCmd.CommandLine -eq $lastCmdletError.Line) {
                $exitCode = 1
            } else {
                $exitCode = $origLastExitCode
            }
        }
    }

    $result = $null
    try {
        $keymap = if ([Microsoft.PowerShell.PSConsoleReadLine]::InViCommandMode()) {
            "vi"
        } else {
            "emacs"
        }
        $result = Get-StarshipPrompt -ExitCode $exitCode -Keymap $keymap -Width $Host.UI.RawUI.WindowSize.Width
    } catch {}

    if (-not $result) {
        $loc = $executionContext.SessionState.Path.CurrentLocation
        $result = "PS $($loc.ProviderPath)> "
    }

    $result

    $global:LASTEXITCODE = $origLastExitCode

    if ($global:? -ne $origDollarQuestion) {
        if ($origDollarQuestion) {
            1 + 1
        } else {
            Write-Error '' -ErrorAction 'Ignore'
        }
    }
}

Export-ModuleMember -Function global:prompt, Start-StarshipDaemon, Stop-StarshipDaemon, Restart-StarshipDaemon, Get-StarshipDaemonTimings
