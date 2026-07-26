$dotfiles = "$env:USERPROFILE\Documents\dotfiles"
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>$null

$status = 0; $keymap = "vi"; $width = 120; $cfg = $env:STARSHIP_CONFIG

# Benchmark A: Old JSON (string concat + Substring for starship_config)
$timesA = @()
for ($i = 0; $i -lt 1000; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $j = '{"status_code":' + $status + ',"keymap":"' + $keymap + '","terminal_width":' + $width + '}'
    if ($cfg) { $j = $j.Substring(0, $j.Length - 1) + ',"starship_config":"' + $cfg + '"}' }
    $null = [Text.Encoding]::UTF8.GetBytes($j)
    $sw.Stop()
    $timesA += $sw.Elapsed.TotalMilliseconds
}
$sA = $timesA | Sort-Object

# Benchmark B: ConvertTo-Json
$props = @{status_code=$status; keymap=$keymap; terminal_width=$width; starship_config=$cfg}
$timesB = @()
for ($i = 0; $i -lt 1000; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $j = $props | ConvertTo-Json -Compress
    $null = [Text.Encoding]::UTF8.GetBytes($j)
    $sw.Stop()
    $timesB += $sw.Elapsed.TotalMilliseconds
}
$sB = $timesB | Sort-Object

Write-Host "=== JSON construction (1000x each) ==="
Write-Host ("Old (string concat + Substring): min={0:N4}ms  p50={1:N4}ms" -f $sA[0], $sA[500])
Write-Host ("New (ConvertTo-Json -Compress): min={0:N4}ms  p50={1:N4}ms" -f $sB[0], $sB[500])
Write-Host ("Ratio: {0:N1}x slower" -f ($sB[500] / $sA[500]))

# Benchmark C: Daemon start time with readiness check
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>$null
Start-Sleep 2

$swTotal = [System.Diagnostics.Stopwatch]::StartNew()
$null = Start-Process -FilePath "C:\Users\Dong\Documents\code\starship-daemon\target\release\starship-daemon.exe" -WindowStyle Hidden
for ($i = 0; $i -lt 5; $i++) {
    try {
        $test = [System.IO.Pipes.NamedPipeClientStream]::new(".", "starship-daemon")
        $test.Connect(100)
        $test.Dispose()
        break
    } catch { Start-Sleep -Milliseconds 100 }
}
$swTotal.Stop()
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>$null

Write-Host "`n=== Daemon start ==="
Write-Host ("Start-Process + readiness check: {0:N1}ms" -f $swTotal.Elapsed.TotalMilliseconds)
