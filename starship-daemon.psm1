$script:DaemonPath = $env:STARSHIP_DAEMON_PATH
$script:LastStarshipConfig = $null
$script:DaemonPipe = $null

function Start-StarshipDaemon
{
    if ([string]::IsNullOrEmpty($script:DaemonPath))
    { return }
    if ((Get-Process -Name starship-daemon -ErrorAction SilentlyContinue).Count -eq 0)
    {
        $null = Start-Process -FilePath $script:DaemonPath -WindowStyle Hidden
    }
}

function Get-StarshipPrompt
{
    param([int]$ExitCode, [string]$Keymap, [int]$Width)

    for ($attempt = 0; $attempt -lt 2; $attempt++)
    {
        $pipe = $script:DaemonPipe
        if ($null -eq $pipe)
        {
            $pipe = try
            {
                $pipeName = if ($env:STARSHIP_DAEMON_PIPE) { $env:STARSHIP_DAEMON_PIPE } else { "starship-daemon" }
                $p = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeName)
                $p.Connect(10); $p
            } catch
            {
                return $null
            }
            $script:DaemonPipe = $pipe
        }

        $cwd = $PWD.ProviderPath
        $cwdBytes = [Text.Encoding]::UTF8.GetBytes($cwd)

        if ($cwdBytes.Length -gt 32768)
        {
            return $null
        }

        $failed = $false
        $result = $null
        try
        {
            $keymapBytes = [Text.Encoding]::UTF8.GetBytes($Keymap)
            $currentCfg = $env:STARSHIP_CONFIG
            $cfgChanged = $currentCfg -ne $script:LastStarshipConfig
            if ($cfgChanged)
            {
                $configBytes = [Text.Encoding]::UTF8.GetBytes($currentCfg)
            } else
            {
                $configBytes = [byte[]]::new(0)
            }
            $disableCache = 0
            if ($env:STARSHIP_DAEMON_CACHE -eq "0")
            { $disableCache = 1 }

            $bodyLen = 4 + $cwdBytes.Length + 4 + 2 + $keymapBytes.Length + 4 + 2 + $configBytes.Length + 1
            $buf = [byte[]]::new(5 + $bodyLen)
            $buf[0] = 1
            [BitConverter]::GetBytes([uint32]$bodyLen).CopyTo($buf, 1)
            $o = 5
            [BitConverter]::GetBytes([uint32]$cwdBytes.Length).CopyTo($buf, $o); $o += 4
            $cwdBytes.CopyTo($buf, $o); $o += $cwdBytes.Length
            [BitConverter]::GetBytes([int32]$ExitCode).CopyTo($buf, $o); $o += 4
            [BitConverter]::GetBytes([uint16]$keymapBytes.Length).CopyTo($buf, $o); $o += 2
            $keymapBytes.CopyTo($buf, $o); $o += $keymapBytes.Length
            [BitConverter]::GetBytes([uint32]$Width).CopyTo($buf, $o); $o += 4
            [BitConverter]::GetBytes([uint16]$configBytes.Length).CopyTo($buf, $o); $o += 2
            $configBytes.CopyTo($buf, $o); $o += $configBytes.Length
            $buf[$o] = [byte]$disableCache
            $pipe.Write($buf, 0, $buf.Length)
            $pipe.Flush()

            $lenBuf = [byte[]]::new(4)
            if ($pipe.Read($lenBuf, 0, 4) -ne 4)
            {
                $failed = $true
            } else
            {
                $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
                if ($respLen -le 0 -or $respLen -gt 65536)
                {
                    $failed = $true
                } else
                {
                    $respBuf = [byte[]]::new($respLen)
                    $read = 0
                    while ($read -lt $respLen)
                    {
                        $n = $pipe.Read($respBuf, $read, $respLen - $read)
                        if ($n -le 0)
                        {
                            $failed = $true
                            break
                        }
                        $read += $n
                    }
                    if (-not $failed)
                    {
                        $result = [Text.Encoding]::UTF8.GetString($respBuf, 0, $read)
                    }
                }
            }
        } catch
        {
            $failed = $true
        }

        if ($failed)
        {
            $pipe.Dispose()
            $script:DaemonPipe = $null
            if ($attempt -eq 0)
            { continue }
            return $null
        }

        if ($cfgChanged)
        {
            $script:LastStarshipConfig = $currentCfg
        }
        return $result
    }
    return $null
}

function Disable-StarshipDaemon
{
    if ($null -ne $script:DaemonPipe)
    {
        $script:DaemonPipe.Dispose()
        $script:DaemonPipe = $null
    }
    Get-Process -Name starship-daemon -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

$MyInvocation.MyCommand.ScriptBlock.Module.OnRemove = { Disable-StarshipDaemon }

$env:VIRTUAL_ENV_DISABLE_PROMPT = 1
$env:STARSHIP_SHELL = if ($PSVersionTable.PSVersion.Major -gt 5) { "pwsh" } else { "powershell" }

Start-StarshipDaemon

Set-PSReadLineOption -ContinuationPrompt "· "

function global:prompt
{
    $origDollarQuestion = $global:?
    $origLastExitCode = $global:LASTEXITCODE

    try
    {
        if (Test-Path function:Invoke-Starship-PreCommand)
        {
            Invoke-Starship-PreCommand
        }
    } catch { }

    $loc = $executionContext.SessionState.Path.CurrentLocation

    $exitCode = 0
    if ($lastCmd = Get-History -Count 1)
    {
        if (-not $origDollarQuestion)
        {
            $lastCmdletError = try { $global:error[0] | Where-Object { $_ -ne $null } | Select-Object -ExpandProperty InvocationInfo } catch { $null }
            $exitCode = if ($null -ne $lastCmdletError -and $lastCmd.CommandLine -eq $lastCmdletError.Line) { 1 } else { $origLastExitCode }
        }
    }

    $result = $null
    try
    {
        $keymap = if ([Microsoft.PowerShell.PSConsoleReadLine]::InViCommandMode()) { "vi" } else { "emacs" }
        $result = Get-StarshipPrompt -ExitCode $exitCode -Keymap $keymap -Width $Host.UI.RawUI.WindowSize.Width
    } catch
    {
    }

    if (-not $result)
    {
        $result = "PS $($loc.ProviderPath)> "
    }

    Set-PSReadLineOption -ExtraPromptLineCount ($result.Split("`n").Length - 1)

    $result

    $global:LASTEXITCODE = $origLastExitCode

    if ($global:? -ne $origDollarQuestion)
    {
        if ($origDollarQuestion)
        {
            1+1
        } else
        {
            Write-Error '' -ErrorAction 'Ignore'
        }
    }
}
