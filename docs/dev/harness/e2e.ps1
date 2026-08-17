param([int]$BudgetSeconds = 8, [int]$WarmSamples = 40, [string]$WarmOut = "")
$ErrorActionPreference = "Stop"
$repo = Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent
Set-Location $repo
function QPC-Us { [double][System.Diagnostics.Stopwatch]::GetTimestamp() / [System.Diagnostics.Stopwatch]::Frequency * 1e6 }

# --- real module E2E ---
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PATH = "$repo\target\release\starship-daemon.exe"   # daemon defers opt-out default
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400
Import-Module "$repo\starship-daemon.psm1" -Force
$script:_mod = Get-Module starship-daemon
# warm up module JIT + daemon
for ($i=0;$i-lt 25;$i++){ $null = & $script:_mod { param($ec,$km,$wd) Get-StarshipPrompt -ExitCode $ec -Keymap $km -Width $wd } 0 "emacs" 120 }

$script:_mod = Get-Module starship-daemon
# function to run one REAL module call (internal fn via module scope) timed
function Invoke-RealPrompt {
    $t0 = QPC-Us
    $r = & $script:_mod { param($ec,$km,$wd) Get-StarshipPrompt -ExitCode $ec -Keymap $km -Width $wd } 0 "emacs" 120
    $t1 = QPC-Us
    if ($null -eq $r) { throw "module returned null" }
    return ($t1 - $t0)
}

Write-Host "E2E (real Get-StarshipPrompt): idle 3s/sample until ${BudgetSeconds}s..."
$ps = New-Object System.Collections.Generic.List[double]
$n = 0
$sw = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt $BudgetSeconds) {
    Start-Sleep -Milliseconds 3000
    $ps.Add((Invoke-RealPrompt)); $n++
}
$pw = New-Object System.Collections.Generic.List[double]
for ($i=0;$i-lt $WarmSamples;$i++){ $pw.Add((Invoke-RealPrompt)) }
if ($WarmOut) { $pw | ForEach-Object { "{0:F3}" -f $_ } | Set-Content $WarmOut }

function Med2([double[]]$a){ $s=$a|Sort-Object; ($s[[int]($s.Count/2)]) }
function Trim2([double[]]$a){ $s=$a|Sort-Object; $t=[int]($s.Count*0.10); ($s[$t..($s.Count-1-$t)]|Measure-Object -Average).Average }
Write-Host ("  cold samples: {0}" -f $n)
Write-Host ("  REAL E2E cold  : med={0,6} us  trim10={1,6} us" -f [math]::Round((Med2 $ps)), [math]::Round((Trim2 $ps)))
Write-Host ("  REAL E2E warm  : med={0,6} us" -f [math]::Round((Med2 $pw)))
Write-Host ("  cold overhead vs warm: +{0,6} us" -f [math]::Round((Med2 $ps)-(Med2 $pw)))

# --- per-stage breakdown (replica of Get-StarshipPrompt body on cache-hit cold) ---
Write-Host "`n=== per-stage breakdown (module body replica, cache-hit, cold idle=you) ==="
# rebuild a fresh pipe to mirror module cache-hit? reuse $script:DaemonPipe already cached = cache-hit.
$keymap = "emacs"; $Width = 120
$cwdS = New-Object System.Collections.Generic.List[double]
$bdS  = New-Object System.Collections.Generic.List[double]
$wrS  = New-Object System.Collections.Generic.List[double]
$rdS  = New-Object System.Collections.Generic.List[double]
$okN  = 0
$stageN = [Math]::Min($n, 40)
for ($i=0;$i-lt $stageN;$i++){
    Start-Sleep -Milliseconds 3000
    $t0=QPC-Us; $cwd=$PWD.ProviderPath; $t1=QPC-Us                                  # ProviderPath
    $tb=QPC-Us; $buf=[StarshipFrame]::Build($cwd,0,$keymap,$Width,$null,[byte]0); $t2=QPC-Us  # Build
    $tw=QPC-Us; $pipe=& $script:_mod { $script:DaemonPipe }; $pipe.Write($buf,0,$buf.Length); $pipe.Flush(); $t3=QPC-Us # Write
    $resp=[byte[]]::new(65536); $tr=QPC-Us; $rn=$pipe.Read($resp,0,$resp.Length); $t4=QPC-Us      # Read
    $null=[StarshipFrame]::Parse($resp,$rn)
    $cwdS.Add($t1-$t0); $bdS.Add($t2-$tb); $wrS.Add($t3-$tw); $rdS.Add($t4-$tr)
}
Write-Host ("  providerpath : med={0,6} us" -f [math]::Round((Med2 $cwdS)))
Write-Host ("  frame build  : med={0,6} us" -f [math]::Round((Med2 $bdS)))
Write-Host ("  pipe write   : med={0,6} us" -f [math]::Round((Med2 $wrS)))
Write-Host ("  pipe read    : med={0,6} us" -f [math]::Round((Med2 $rdS)))
$sum = [math]::Round((Med2 $cwdS)+(Med2 $bdS)+(Med2 $wrS)+(Med2 $rdS))
Write-Host ("  stage sum    : med={0,6} us" -f $sum)

# cleanup
Remove-Module starship-daemon
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_PATH -ErrorAction SilentlyContinue
Write-Host "DONE"
