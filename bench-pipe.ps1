param(
    [int]$Runs = 50
)

$daemonPath = "C:\Users\Dong\Documents\code\starship-daemon\target\release\starship-daemon.exe"
$cwdPath = "C:\Users\Dong\Documents\dotfiles"
$props = @{status_code=0;keymap="vi";cmd_duration_ms=$null;jobs=0;shlvl=$null;terminal_width=120} | ConvertTo-Json -Compress
$cwdBytes = [Text.Encoding]::UTF8.GetBytes($cwdPath)
$propsBytes = [Text.Encoding]::UTF8.GetBytes($props)
$request = [byte[]]::new(8 + $cwdBytes.Length + $propsBytes.Length)
[BitConverter]::GetBytes([uint32]$cwdBytes.Length).CopyTo($request, 0)
$cwdBytes.CopyTo($request, 4)
[BitConverter]::GetBytes([uint32]$propsBytes.Length).CopyTo($request, 4 + $cwdBytes.Length)
$propsBytes.CopyTo($request, 8 + $cwdBytes.Length)

# Warmup
$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "starship-daemon")
$pipe.Connect(2000)
$pipe.Write($request, 0, $request.Length)
$pipe.Flush()
$lenBuf = [byte[]]::new(4)
$pipe.Read($lenBuf, 0, 4) | Out-Null
$respLen = [BitConverter]::ToUInt32($lenBuf, 0)
$respBuf = [byte[]]::new($respLen)
$pipe.Read($respBuf, 0, $respLen) | Out-Null
$pipe.Dispose()

# Benchmark
$times = [System.Collections.ArrayList]::new()
for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "starship-daemon")
    $pipe.Connect(2000)
    $pipe.Write($request, 0, $request.Length)
    $pipe.Flush()
    $pipe.Read($lenBuf, 0, 4) | Out-Null
    $respLen = [BitConverter]::ToUInt32($lenBuf, 0)
    $respBuf = [byte[]]::new($respLen)
    $pipe.Read($respBuf, 0, $respLen) | Out-Null
    $pipe.Dispose()
    $sw.Stop()
    [void]$times.Add($sw.Elapsed.TotalMilliseconds)
}

Write-Host "=== Pipe round-trip ($Runs runs) ==="
Write-Host ("  min: {0:N3}ms" -f ($times | Measure-Object -Minimum).Minimum)
Write-Host ("  avg: {0:N3}ms" -f ($times | Measure-Object -Average).Average)
Write-Host ("  max: {0:N3}ms" -f ($times | Measure-Object -Maximum).Maximum)
$sorted = $times | Sort-Object
Write-Host ("  p50: {0:N3}ms" -f $sorted[[int]($sorted.Count/2)])
Write-Host ("Request: {0} bytes" -f $request.Length)

# Compare: starship prompt subprocess
$times2 = [System.Collections.ArrayList]::new()
$subRuns = [math]::Min(20, $Runs)
for ($i = 0; $i -lt $subRuns; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $null = & starship prompt --path="$cwdPath" --terminal-width=120 --status=0 --keymap=vi 2>&1 | Out-String
    $sw.Stop()
    [void]$times2.Add($sw.Elapsed.TotalMilliseconds)
}

Write-Host ""
Write-Host "=== starship prompt subprocess ($subRuns runs) ==="
Write-Host ("  min: {0:N3}ms" -f ($times2 | Measure-Object -Minimum).Minimum)
Write-Host ("  avg: {0:N3}ms" -f ($times2 | Measure-Object -Average).Average)
Write-Host ("  max: {0:N3}ms" -f ($times2 | Measure-Object -Maximum).Maximum)
