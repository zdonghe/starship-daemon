Get-Process starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 200

$env:STARSHIP_CONFIG = "$env:USERPROFILE\Documents\dotfiles\configs\starship\starship.toml"
$env:STARSHIP_DAEMON_PATH = "$env:USERPROFILE\Documents\code\starship-daemon\target\release\starship-daemon.exe"

Import-Module "$env:USERPROFILE\Documents\dotfiles\configs\Powershell\starship-daemon.psm1" -DisableNameChecking

function global:prompt
{
    $lastExit = $LASTEXITCODE
    if ($lastExit -eq $null)
    { $lastExit = 0 
    }
    $keymap = ''
    $width  = $Host.UI.RawUI.WindowSize.Width
    $result = Get-StarshipPrompt -ExitCode $lastExit -Keymap $keymap -Width $width
    if ($result)
    { $result 
    } else
    { "PS> " 
    }
}
$Warmup  = 15
$Samples = 200

1..$Warmup | ForEach-Object { prompt | Out-Null }

$nullCount = 0
$times = 1..$Samples | ForEach-Object {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = prompt
    $sw.Stop()
    if ([string]::IsNullOrEmpty($r))
    { $script:nullCount++ 
    }
    $sw.Elapsed.TotalMilliseconds
}
Write-Host "Null/fallback responses: $nullCount / $Samples"

$sorted = $times | Sort-Object
[PSCustomObject]@{
    Impl       = 'cache'
    Mean       = [math]::Round(($times | Measure-Object -Average).Average, 2)
    Median     = $sorted[$sorted.Count / 2]
    P95        = $sorted[[int]($sorted.Count * 0.95)]
    Min        = $sorted[0]
    Max        = $sorted[-1]
    NullCount  = $nullCount
} | Export-Csv "$PSScriptRoot\result-cache.csv" -NoTypeInformation
