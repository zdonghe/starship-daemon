param(
    [string]$ModulePath,
    [string]$DaemonPipe,
    [string]$CtrlPipe,
    [string]$ReadyFile,
    [string]$OutFile,
    [int]$Warm = 30,
    [int]$N = 150
)
$ErrorActionPreference = "Stop"
$repo = (Split-Path $PSScriptRoot -Parent)
function QPC-Us { [double][System.Diagnostics.Stopwatch]::GetTimestamp() / [System.Diagnostics.Stopwatch]::Frequency * 1e6 }

Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_CACHE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PATH = "$repo\target\release\starship-daemon.exe"
$env:STARSHIP_DAEMON_PIPE = $DaemonPipe

Import-Module PSReadLine -ErrorAction SilentlyContinue
Import-Module $ModulePath -Force

for ($i = 0; $i -lt $Warm; $i++) {
    $null = global:prompt
}

$srv = [System.IO.Pipes.NamedPipeServerStream]::new($CtrlPipe, [System.IO.Pipes.PipeDirection]::InOut)
Set-Content -LiteralPath $ReadyFile -Value "ready"
$srv.WaitForConnection()
$rdr = [System.IO.StreamReader]::new($srv)
$wtr = [System.IO.StreamWriter]::new($srv)
$wtr.AutoFlush = $true
$samples = [System.Collections.Generic.List[string]]::new()

for ($i = 0; $i -lt $N; $i++) {
    $cmd = $rdr.ReadLine()
    if ($cmd -eq "quit") { break }
    if ($cmd -ne "go") { continue }
    $t0 = QPC-Us
    $r = global:prompt
    $t1 = QPC-Us
    if (-not $r) { throw "global:prompt returned empty" }
    $samples.Add(("{0:F3}" -f ($t1 - $t0)))
    $wtr.WriteLine("done")
}

$samples | Set-Content -LiteralPath $OutFile
$rdr.Dispose(); $wtr.Dispose(); $srv.Dispose()