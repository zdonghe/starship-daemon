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

Import-Module $ModulePath -Force
$mod = Get-Module starship-daemon
for ($i = 0; $i -lt $Warm; $i++) {
    $null = & $mod { param($ec, $km, $wd) Get-StarshipPrompt -ExitCode $ec -Keymap $km -Width $wd } 0 "emacs" 120
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
    $r = & $mod { param($ec, $km, $wd) Get-StarshipPrompt -ExitCode $ec -Keymap $km -Width $wd } 0 "emacs" 120
    $t1 = QPC-Us
    if ($null -eq $r) { throw "module returned null" }
    $samples.Add(("{0:F3}" -f ($t1 - $t0)))
    try { $wtr.WriteLine("done") } catch { }
}
try { $rdr.Dispose() } catch { }
try { $wtr.Dispose() } catch { }
try { $srv.Dispose() } catch { }

Remove-Module starship-daemon
$samples | Set-Content -LiteralPath $OutFile
