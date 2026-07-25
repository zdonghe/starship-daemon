$dotfiles = "C:\Users\Dong\Documents\dotfiles"

# ---------- Helper: send one prompt request, return elapsed ms ----------
function Send-Request($cwd, $status=0) {
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "starship-daemon")
    $pipe.Connect(2000)
    $props = @{status_code=$status;keymap="vi"} | ConvertTo-Json -Compress
    $cwdBytes = [Text.Encoding]::UTF8.GetBytes($cwd)
    $propsBytes = [Text.Encoding]::UTF8.GetBytes($props)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $pipe.Write([BitConverter]::GetBytes([uint32]$cwdBytes.Length), 0, 4)
    $pipe.Write($cwdBytes, 0, $cwdBytes.Length)
    $pipe.Write([BitConverter]::GetBytes([uint32]$propsBytes.Length), 0, 4)
    $pipe.Write($propsBytes, 0, $propsBytes.Length)
    $pipe.Flush()
    $lenBuf = [byte[]]::new(4)
    $pipe.Read($lenBuf, 0, 4) | Out-Null
    $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
    $respBuf = [byte[]]::new($respLen)
    $pipe.Read($respBuf, 0, $respLen) | Out-Null
    $pipe.Dispose()
    $sw.Stop()
    return $sw.Elapsed.TotalMilliseconds
}

# Kill existing daemon, start fresh
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>/dev/null
Start-Sleep -Seconds 2
$env:STARSHIP_CONFIG = "$dotfiles\configs\starship\starship.toml"
Start-Process -FilePath "C:\Users\Dong\Documents\code\starship-daemon\target\release\starship-daemon.exe" -WindowStyle Hidden
Start-Sleep -Seconds 3

Write-Host "=== Scenario 1: Cache hit (same cwd, same status) ==="
$times1 = @()
for ($i = 0; $i -lt 20; $i++) {
    $times1 += Send-Request -cwd $dotfiles -status 0
}
$s1 = $times1 | Sort-Object
Write-Host ("  min={0:N3}ms  p50={1:N3}ms  avg={2:N3}ms  max={3:N3}ms" -f $s1[0], $s1[10], ($s1 | Measure-Object -Average).Average, $s1[-1])

Write-Host "=== Scenario 2: Cache miss (new cwd each time) ==="
$times2 = @()
$repos = @(
    "$dotfiles",
    "$dotfiles\configs\git-fast",
    "$dotfiles\configs\starship-daemon",
    "$env:USERPROFILE\.cargo",
    "$env:USERPROFILE\AppData\Local"
)
for ($i = 0; $i -lt 20; $i++) {
    $c = $repos[$i % $repos.Count]
    $times2 += Send-Request -cwd $c -status ($i % 256)
}
$s2 = $times2 | Sort-Object
Write-Host ("  min={0:N3}ms  p50={1:N3}ms  avg={2:N3}ms  max={3:N3}ms" -f $s2[0], $s2[10], ($s2 | Measure-Object -Average).Average, $s2[-1])

Write-Host "=== Scenario 3: starship subprocess (current daemonless) ==="
$times3 = @()
for ($i = 0; $i -lt 10; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $null = & starship prompt --path="$dotfiles" --terminal-width=120 --status=0 --keymap=vi 2>&1 | Out-String
    $sw.Stop()
    $times3 += $sw.Elapsed.TotalMilliseconds
}
$s3 = $times3 | Sort-Object
Write-Host ("  min={0:N3}ms  p50={1:N3}ms  avg={2:N3}ms  max={3:N3}ms" -f $s3[0], $s3[5], ($s3 | Measure-Object -Average).Average, $s3[-1])

Write-Host ""
Write-Host "Subprocess vs daemon cache hit: $([math]::Round($s3[0] / $s1[0], 0))x faster"
Write-Host "Subprocess vs daemon cache miss: $([math]::Round($s3[0] / $s2[0], 0))x faster"

Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>/dev/null
