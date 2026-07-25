$dotfiles = "C:\Users\Dong\Documents\dotfiles"
$daemonPath = "C:\Users\Dong\Documents\code\starship-daemon\target\release\starship-daemon.exe"

# Kill existing
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>/dev/null
Start-Sleep -Seconds 2

# ---------- Benchmark function ----------
function Measure-Daemon($label, $runs=50) {
    $times = @()
    for ($i = 0; $i -lt $runs; $i++) {
        $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "starship-daemon")
        $pipe.Connect(2000)
        $props = @{status_code=0;keymap="vi"} | ConvertTo-Json -Compress
        $cwdBytes = [Text.Encoding]::UTF8.GetBytes($dotfiles)
        $propsBytes = [Text.Encoding]::UTF8.GetBytes($props)
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $pipe.Write([BitConverter]::GetBytes([uint32]$cwdBytes.Length), 0, 4)
        $pipe.Write($cwdBytes, 0, $cwdBytes.Length)
        $pipe.Write([BitConverter]::GetBytes([uint32]$propsBytes.Length), 0, 4)
        $pipe.Write($propsBytes, 0, $propsBytes.Length)
        $pipe.Flush()
        $lenBuf = [byte[]]::new(4); $pipe.Read($lenBuf, 0, 4) | Out-Null
        $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
        $respBuf = [byte[]]::new($respLen); $pipe.Read($respBuf, 0, $respLen) | Out-Null
        $pipe.Dispose()
        $sw.Stop()
        $times += $sw.Elapsed.TotalMilliseconds
    }
    $s = $times | Sort-Object
    Write-Host ("{0,-35} min={1,8:N3}ms  p50={2,8:N3}ms  avg={3,8:N3}ms  max={4,8:N3}ms" -f $label, $s[0], $s[$runs/2], ($times | Measure-Object -Average).Average, $s[-1])
}

# --- Test 1: Daemon WITH git-fast watcher (current) ---
$env:STARSHIP_CONFIG = "$dotfiles\configs\starship\starship.toml"
Start-Process -FilePath $daemonPath -WindowStyle Hidden
Start-Sleep -Seconds 3
Measure-Daemon "WITH git-fast (cache hit, same cwd)" 50

# Test with different cwds to trigger find_or_create
$timesMiss = @()
$dirs = @( "$dotfiles", "$dotfiles\configs\git-fast", "$dotfiles\configs\starship", "$env:USERPROFILE", "$env:USERPROFILE\.cargo", "$env:USERPROFILE\Desktop", "$env:USERPROFILE\Downloads", "$env:USERPROFILE\Documents" )
for ($i = 0; $i -lt 40; $i++) {
    $c = $dirs[$i % $dirs.Count]
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "starship-daemon")
    $pipe.Connect(2000)
    $props = @{status_code=$i;keymap="vi"} | ConvertTo-Json -Compress
    $cwdBytes = [Text.Encoding]::UTF8.GetBytes($c)
    $propsBytes = [Text.Encoding]::UTF8.GetBytes($props)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $pipe.Write([BitConverter]::GetBytes([uint32]$cwdBytes.Length), 0, 4)
    $pipe.Write($cwdBytes, 0, $cwdBytes.Length)
    $pipe.Write([BitConverter]::GetBytes([uint32]$propsBytes.Length), 0, 4)
    $pipe.Write($propsBytes, 0, $propsBytes.Length)
    $pipe.Flush()
    $lenBuf = [byte[]]::new(4); $pipe.Read($lenBuf, 0, 4) | Out-Null
    $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
    $respBuf = [byte[]]::new($respLen); $pipe.Read($respBuf, 0, $respLen) | Out-Null
    $pipe.Dispose()
    $sw.Stop()
    $timesMiss += $sw.Elapsed.TotalMilliseconds
}
$sm = $timesMiss | Sort-Object
Write-Host ("{,-35} min={,8:N3}ms  p50={,8:N3}ms  avg={,8:N3}ms  max={,8:N3}ms" -f "WITH git-fast (8 dirs, varying status)", $sm[0], $sm[20], ($timesMiss | Measure-Object -Average).Average, $sm[-1])

# Kill daemon
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force 2>/dev/null
Start-Sleep -Seconds 2

# --- Test 2: starship subprocess ---
Write-Host ""
Write-Host "=== Baseline: starship prompt subprocess (no daemon) ==="
$timesSub = @()
for ($i = 0; $i -lt 10; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $null = & starship prompt --path="$dotfiles" --terminal-width=120 --status=0 --keymap=vi 2>&1 | Out-String
    $sw.Stop()
    $timesSub += $sw.Elapsed.TotalMilliseconds
}
$ss = $timesSub | Sort-Object
Write-Host ("{,-35} min={,8:N3}ms  p50={,8:N3}ms  avg={,8:N3}ms  max={,8:N3}ms" -f "starship prompt subprocess", $ss[0], $ss[5], ($timesSub | Measure-Object -Average).Average, $ss[-1])

Write-Host ""
Write-Host "Note: git-fast uses gix 0.75, starship uses gix 0.85."
Write-Host "They are separate libraries with separate caches."
Write-Host "The watcher provides zero benefit to get_prompt()."
