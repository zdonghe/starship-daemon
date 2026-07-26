# starship-daemon.psm1
# Drop-in daemon management for starship prompt — Import-Module, auto-start, clean unload.
#
# Usage:  Import-Module starship-daemon
# Remove: Remove-Module starship-daemon  (or Disable-StarshipDaemon)

$script:DaemonPath = $env:STARSHIP_DAEMON_PATH
$script:LastStarshipConfig = $null

function Start-StarshipDaemon
{
    if ([string]::IsNullOrEmpty($script:DaemonPath)) { return }
    if ((Get-Process -Name starship-daemon -ErrorAction SilentlyContinue).Count -eq 0)
    {
        $null = Start-Process -FilePath $script:DaemonPath -WindowStyle Hidden
    }
}

function Get-StarshipPrompt
{
    param([int]$ExitCode, [string]$Keymap, [int]$Width)

    $pipe = try
    {
        $p = [System.IO.Pipes.NamedPipeClientStream]::new(".", "starship-daemon")
        $p.Connect(10); $p
    } catch
    { return $null 
    }

    $cwd = (Get-Location).ProviderPath
    $cwdBytes = [Text.Encoding]::UTF8.GetBytes($cwd)

    $propsJson = '{"status_code":' + $ExitCode + ',"keymap":"' + $Keymap + '","terminal_width":' + $Width

    $currentCfg = $env:STARSHIP_CONFIG
    if ($currentCfg -ne $script:LastStarshipConfig)
    {
        $propsJson += ',"starship_config":"' + $currentCfg + '"'
        $script:LastStarshipConfig = $currentCfg
    }

    if ($env:STARSHIP_DAEMON_CACHE -eq "0")
    {
        $propsJson += ',"disable_cache":true'
    }

    $propsJson += '}'
    $propsBytes = [Text.Encoding]::UTF8.GetBytes($propsJson)

    $pipe.Write([BitConverter]::GetBytes([uint32]$cwdBytes.Length), 0, 4)
    $pipe.Write($cwdBytes, 0, $cwdBytes.Length)
    $pipe.Write([BitConverter]::GetBytes([uint32]$propsBytes.Length), 0, 4)
    $pipe.Write($propsBytes, 0, $propsBytes.Length)
    $pipe.Flush()

    $lenBuf = [byte[]]::new(4)
    $result = $null
    if ($pipe.Read($lenBuf, 0, 4) -eq 4)
    {
        $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
        if ($respLen -gt 0 -and $respLen -le 65536)
        {
            $respBuf = [byte[]]::new($respLen)
            $read = 0
            while ($read -lt $respLen)
            {
                $n = $pipe.Read($respBuf, $read, $respLen - $read)
                if ($n -le 0)
                { break 
                }
                $read += $n
            }
            $result = [Text.Encoding]::UTF8.GetString($respBuf, 0, $read)
        }
    }
    $pipe.Dispose()
    return $result
}

function Disable-StarshipDaemon
{
    Get-Process -Name starship-daemon -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

$MyInvocation.MyCommand.ScriptBlock.Module.OnRemove = { Disable-StarshipDaemon }

# Auto-start daemon on import
Start-StarshipDaemon

# Set continuation prompt (multi-line input prefix) to match starship default
Set-PSReadLineOption -ContinuationPrompt "· "

function global:prompt {
    $lastCmdOk = if ($null -ne $global:PromptLastCmdOk) { $global:PromptLastCmdOk } else { $? }
    $origLastExitCode = if ($null -ne $global:PromptLastExitCode) { $global:PromptLastExitCode } else { $global:LASTEXITCODE }
    $loc = $executionContext.SessionState.Path.CurrentLocation

    try {
        $keymap = if ([Microsoft.PowerShell.PSConsoleReadLine]::InViCommandMode()) { "vi" } else { "emacs" }
        $exitCode = if ($lastCmdOk) { 0 } elseif ($origLastExitCode -ne 0) { $origLastExitCode } else { 1 }
        $result = Get-StarshipPrompt -ExitCode $exitCode -Keymap $keymap -Width $Host.UI.RawUI.WindowSize.Width
        if ($result) { return $result }
    } catch {}

    "PS $($loc.ProviderPath)> "
}
