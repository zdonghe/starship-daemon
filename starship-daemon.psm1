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

        $cwd = [System.IO.Directory]::GetCurrentDirectory()
        $cwdBytes = [Text.Encoding]::UTF8.GetBytes($cwd)

        $propsJson = '{"status_code":' + $ExitCode + ',"keymap":"' + $Keymap + '","terminal_width":' + $Width

        $currentCfg = $env:STARSHIP_CONFIG
        $cfgChanged = $currentCfg -ne $script:LastStarshipConfig
        if ($cfgChanged)
        {
            $propsJson += ',"starship_config":"' + $currentCfg + '"'
        }

        if ($env:STARSHIP_DAEMON_CACHE -eq "0")
        {
            $propsJson += ',"disable_cache":true'
        }

        $propsJson += '}'
        $propsBytes = [Text.Encoding]::UTF8.GetBytes($propsJson)

        $failed = $false
        $result = $null
        try
        {
            $buf = [byte[]]::new(4 + $cwdBytes.Length + 4 + $propsBytes.Length)
            [BitConverter]::GetBytes([uint32]$cwdBytes.Length).CopyTo($buf, 0)
            $cwdBytes.CopyTo($buf, 4)
            [BitConverter]::GetBytes([uint32]$propsBytes.Length).CopyTo($buf, 4 + $cwdBytes.Length)
            $propsBytes.CopyTo($buf, 4 + $cwdBytes.Length + 4)
            $pipe.Write($buf, 0, $buf.Length)
            $pipe.Flush()

            $lenBuf = [byte[]]::new(4)
            if ($pipe.Read($lenBuf, 0, 4) -ne 4)
            {
                $failed = $true
            }
            else
            {
                $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
                if ($respLen -le 0 -or $respLen -gt 65536)
                {
                    $failed = $true
                }
                else
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

# Auto-start daemon on import
Start-StarshipDaemon

# Continuation prompt matching starship default
Set-PSReadLineOption -ContinuationPrompt "· "

function global:prompt
{
    $lastCmdOk = if ($null -ne $global:PromptLastCmdOk)
    { $global:PromptLastCmdOk 
    } else
    { $? 
    }
    $origLastExitCode = if ($null -ne $global:PromptLastExitCode)
    { $global:PromptLastExitCode 
    } else
    { $global:LASTEXITCODE 
    }
    $loc = $executionContext.SessionState.Path.CurrentLocation

    try
    {
        $keymap = if ([Microsoft.PowerShell.PSConsoleReadLine]::InViCommandMode()) { "vi" } else { "emacs" }
        $exitCode = if ($lastCmdOk)
        { 0 
        } elseif ($origLastExitCode -ne 0)
        { $origLastExitCode 
        } else
        { 1 
        }
        $result = Get-StarshipPrompt -ExitCode $exitCode -Keymap $keymap -Width $Host.UI.RawUI.WindowSize.Width
        if ($result)
        { return $result 
        }
    } catch
    {
    }

    "PS $($loc.ProviderPath)> "
}
