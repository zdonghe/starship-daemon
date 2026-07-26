# Set STARSHIP_CONFIG before running, or starship will use its default
if (-not $env:STARSHIP_CONFIG) { $env:STARSHIP_CONFIG = "$env:USERPROFILE\.config\starship.toml" }

# Generate + dot-source starship's actual init script, so `prompt` is the real thing
(& starship init powershell) | Out-String | Invoke-Expression

$Warmup  = 15
$Samples = 200

1..$Warmup | ForEach-Object { prompt | Out-Null }

$times = 1..$Samples | ForEach-Object {
    (Measure-Command { prompt | Out-Null }).TotalMilliseconds
}

$sorted = $times | Sort-Object
[PSCustomObject]@{
    Impl   = 'old'
    Mean   = [math]::Round(($times | Measure-Object -Average).Average, 2)
    Median = $sorted[$sorted.Count / 2]
    P95    = $sorted[[int]($sorted.Count * 0.95)]
    Min    = $sorted[0]
    Max    = $sorted[-1]
} | Export-Csv "$PSScriptRoot\result-old.csv" -NoTypeInformation
