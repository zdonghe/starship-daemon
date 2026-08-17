param([int]$N = 150, [int]$Warm = 30, [int]$IdleMs = 3000, [int]$BudgetSeconds = 0, [string]$Tag = "all", [string]$AModule = "", [string]$BModule = "", [string]$AOut = "", [string]$BOut = "", [string]$Worker = "module_ab_worker.ps1")
$ErrorActionPreference = "Stop"
$repo = Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent
$daemonExe = "$repo\target\release\starship-daemon.exe"
$base = "$PSScriptRoot\ab"
$worker = "$PSScriptRoot\$Worker"
Set-Location $repo
function QPC-Us { [double][System.Diagnostics.Stopwatch]::GetTimestamp() / [System.Diagnostics.Stopwatch]::Frequency * 1e6 }

function NormalCDF([double]$z) {
    $t = 1.0 / (1.0 + 0.2316419 * [math]::Abs($z))
    $d = 0.3989422804014327 * [math]::Exp(-$z * $z / 2.0)
    $p = 1.0 - $d * $t * (0.319381530 + $t * (-0.356563782 + $t * (1.781477937 + $t * (-1.821255978 + $t * 1.330274429))))
    if ($z -lt 0) { return 1.0 - $p }
    return $p
}

function Invoke-MannWhitneyU([double[]]$x, [double[]]$y) {
    $all = @()
    foreach ($v in $x) { $all += [pscustomobject]@{ v = [double]$v; g = 0 } }
    foreach ($v in $y) { $all += [pscustomobject]@{ v = [double]$v; g = 1 } }
    $sorted = $all | Sort-Object v
    $n = $sorted.Count
    $ranks = [double[]]::new($n)
    $i = 0
    while ($i -lt $n) {
        $j = $i
        while ($j + 1 -lt $n -and $sorted[$j + 1].v -eq $sorted[$i].v) { $j++ }
        $avg = (($i + 1) + ($j + 1)) / 2.0
        for ($k = $i; $k -le $j; $k++) { $ranks[$k] = $avg }
        $i = $j + 1
    }
    $n1 = $x.Count; $n2 = $y.Count
    $R1 = 0.0
    for ($i = 0; $i -lt $n; $i++) { if ($sorted[$i].g -eq 0) { $R1 += $ranks[$i] } }
    $U1 = $R1 - $n1 * ($n1 + 1) / 2.0
    $mu = $n1 * $n2 / 2.0
    $sumTies = 0.0
    $i = 0
    while ($i -lt $n) {
        $j = $i
        while ($j + 1 -lt $n -and $sorted[$j + 1].v -eq $sorted[$i].v) { $j++ }
        $len = $j - $i + 1
        if ($len -gt 1) { $sumTies += $len * $len * $len - $len }
        $i = $j + 1
    }
    $var = $n1 * $n2 / 12.0 * (($n + 1) - $sumTies / ($n * ($n - 1)))
    $z = ($U1 - $mu) / [math]::Sqrt($var)
    $p = 2.0 * (1.0 - (NormalCDF ([math]::Abs($z))))
    return $p
}

function Stats([double[]]$a, [string]$label) {
    $s = $a | Sort-Object
    $n = $s.Count
    $med = $s[[math]::Floor($n / 2)]
    $trim = [math]::Max(1, [int]($n * 0.10))
    $trimmed = $s[$trim..($n - $trim - 1)]
    $tmean = ($trimmed | Measure-Object -Average).Average
    $pct = foreach ($q in 0.10, 0.25, 0.50, 0.75, 0.90) { [math]::Round($s[[math]::Floor($q * ($n - 1))]) }
    Write-Host ("{0,-14} n={1,3}  median={2,6:N0}  trim10mean={3,6:N0}  p10={4} p25={5} p50={6} p75={7} p90={8}" -f $label, $n, $med, $tmean, $pct[0], $pct[1], $pct[2], $pct[3], $pct[4])
    return @{ med = $med; tmean = $tmean; s = $s }
}

$pipeA = "starship-mod-a"; $pipeB = "starship-mod-b"
$ctrlA = "starship-ctrl-a"; $ctrlB = "starship-ctrl-b"
$readyA = "$base\ready_a.txt"; $readyB = "$base\ready_b.txt"
$outA = if ($AOut) { $AOut } else { "$base\head_samples.csv" }
$outB = if ($BOut) { $BOut } else { "$base\variant_samples.csv" }
$modA = if ($AModule) { $AModule } else { "$base\head\starship-daemon.psm1" }
$modB = if ($BModule) { $BModule } else { "$base\variant\starship-daemon.psm1" }

Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_CACHE -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PIPE = $pipeA
$pA = Start-Process -FilePath $daemonExe -WorkingDirectory $repo -WindowStyle Hidden -PassThru
$env:STARSHIP_DAEMON_PIPE = $pipeB
$pB = Start-Process -FilePath $daemonExe -WorkingDirectory $repo -WindowStyle Hidden -PassThru
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue

function Wait-Pipe($name) {
    for ($i = 0; $i -lt 50; $i++) {
        try { $q = [System.IO.Pipes.NamedPipeClientStream]::new(".", $name); $q.Connect(200); $q.Dispose(); return } catch { Start-Sleep -Milliseconds 200 }
    }
    throw "pipe $name never appeared"
}
Wait-Pipe $pipeA; Wait-Pipe $pipeB

$env:STARSHIP_DAEMON_PIPE = $pipeA
$logA = "$base\worker_${Tag}_a.log"; $logB = "$base\worker_${Tag}_b.log"
$wA = Start-Process pwsh -ArgumentList "-NoProfile -File `"$worker`" -ModulePath `"$modA`" -DaemonPipe $pipeA -CtrlPipe $ctrlA -ReadyFile `"$readyA`" -OutFile `"$outA`" -Warm $Warm -N $N" -WindowStyle Hidden -RedirectStandardError $logA -RedirectStandardOutput "$base\stdout_a.log" -PassThru
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PIPE = $pipeB
$wB = Start-Process pwsh -ArgumentList "-NoProfile -File `"$worker`" -ModulePath `"$modB`" -DaemonPipe $pipeB -CtrlPipe $ctrlB -ReadyFile `"$readyB`" -OutFile `"$outB`" -Warm $Warm -N $N" -WindowStyle Hidden -RedirectStandardError $logB -RedirectStandardOutput "$base\stdout_b.log" -PassThru
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue

$retry = 0
while ($retry -lt 50 -and (-not (Test-Path $readyA) -or -not (Test-Path $readyB))) { Start-Sleep -Milliseconds 200; $retry++ }
if (-not (Test-Path $readyA)) { throw "worker A never ready" }
if (-not (Test-Path $readyB)) { throw "worker B never ready" }

$cA = [System.IO.Pipes.NamedPipeClientStream]::new(".", $ctrlA); $cA.Connect(5000)
$rA = [System.IO.StreamReader]::new($cA)
$wA2 = [System.IO.StreamWriter]::new($cA); $wA2.AutoFlush = $true
$cB = [System.IO.Pipes.NamedPipeClientStream]::new(".", $ctrlB); $cB.Connect(5000)
$rB = [System.IO.StreamReader]::new($cB)
$wB2 = [System.IO.StreamWriter]::new($cB); $wB2.AutoFlush = $true

$aLabel = if ($AModule) { Split-Path -Leaf (Split-Path $modA) } else { "HEAD" }
$bLabel = Split-Path -Leaf (Split-Path $modB)
Write-Host "A=${aLabel} module, B=${bLabel} ($($bLabel)). N=${N} each, ${IdleMs}ms idle, interleaved A,B..."
$sw = [System.Diagnostics.Stopwatch]::StartNew()
for ($i = 0; $i -lt $N; $i++) {
    if ($BudgetSeconds -gt 0 -and $sw.Elapsed.TotalSeconds -ge $BudgetSeconds) { Write-Host "budget reached at $i"; break }
    Start-Sleep -Milliseconds $IdleMs
    $wA2.WriteLine("go"); $null = $rA.ReadLine()
    Start-Sleep -Milliseconds $IdleMs
    $wB2.WriteLine("go"); $null = $rB.ReadLine()
    if (($i + 1) % 25 -eq 0) { Write-Host ("  {0}/{1}" -f ($i + 1), $N) }
}
Write-Host "done sending; elapsed $([math]::Round($sw.Elapsed.TotalSeconds))s"

$wA.WaitForExit(20000) | Out-Null
$wB.WaitForExit(20000) | Out-Null
$wA2.Dispose(); $rA.Dispose(); $cA.Dispose(); $wB2.Dispose(); $rB.Dispose(); $cB.Dispose()

Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Write-Host "`n=== module A/B cold-call latency (us) ==="
$sA = Get-Content $outA | ForEach-Object { [double]$_ }
$sB = Get-Content $outB | ForEach-Object { [double]$_ }
Write-Host ("A samples: {0}   B samples: {1}" -f $sA.Count, $sB.Count)
$ra = Stats ($sA) ("A (" + $aLabel + ")")
$rb = Stats ($sB) ("B (" + $bLabel + ")")
$p = Invoke-MannWhitneyU ($sA) ($sB)
Write-Host ("`nMann-Whitney U p-value: {0:F4}  (n1={1}, n2={2})" -f $p, $sA.Count, $sB.Count)
if ($p -lt 0.05) {
    Write-Host "=> significant difference (p<0.05)"
    $delta = $rb.med - $ra.med
    if ($delta -lt 0) { Write-Host "=> B is FASTER by $([math]::Round([math]::Abs($delta)))us median (B-A = $([math]::Round($delta)))" }
    else { Write-Host "=> B is SLOWER by $([math]::Round($delta))us median (B-A = $([math]::Round($delta)))" }
} else {
    Write-Host "=> no significant difference (p>=0.05)"
}
Write-Host "DONE"
