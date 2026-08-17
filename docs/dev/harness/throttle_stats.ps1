param([int]$N = 150, [int]$Warm = 30, [int]$IdleMs = 3000)
$ErrorActionPreference = "Stop"
$repo = Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent
Set-Location $repo
$daemonExe = "$repo\target\release\starship-daemon.exe"
function QPC-Us { [double][System.Diagnostics.Stopwatch]::GetTimestamp() / [System.Diagnostics.Stopwatch]::Frequency * 1e6 }

$FrameSrc = @"
using System;
using System.Text;
public static class ThFrame {
    static void W32(byte[] b, int o, uint v) { b[o] = (byte)v; b[o + 1] = (byte)(v >> 8); b[o + 2] = (byte)(v >> 16); b[o + 3] = (byte)(v >> 24); }
    static void W16(byte[] b, int o, ushort v) { b[o] = (byte)v; b[o + 1] = (byte)(v >> 8); }
    public static byte[] Build(string cwd) {
        byte[] cb = Encoding.UTF8.GetBytes(cwd); byte[] kb = Encoding.UTF8.GetBytes("emacs");
        int bl = 17 + cb.Length + kb.Length; byte[] buf = new byte[5 + bl]; buf[0] = 1;
        W32(buf, 1, (uint)bl); int o = 5;
        W32(buf, o, (uint)cb.Length); o += 4; Buffer.BlockCopy(cb, 0, buf, o, cb.Length); o += cb.Length;
        W32(buf, o, 0); o += 4; W16(buf, o, (ushort)kb.Length); o += 2; Buffer.BlockCopy(kb, 0, buf, o, kb.Length); o += kb.Length;
        W32(buf, o, 120); o += 4; W16(buf, o, 0); o += 2; buf[o] = 0; return buf;
    }
    public static bool Ok(byte[] b, int r) { if (r < 4) return false; int len = (int)(b[0]|(b[1]<<8)|(b[2]<<16)|(b[3]<<24)); return len>0 && len<=65531 && r>=4+len; }
}
"@
Add-Type -TypeDefinition $FrameSrc

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
    Write-Host ("{0,-16} n={1,3}  median={2,6:N0}  trim10mean={3,6:N0}  p10={4} p25={5} p50={6} p75={7} p90={8}  min={9,6:N0}  max={10,8:N0}" -f $label, $n, $med, $tmean, $pct[0], $pct[1], $pct[2], $pct[3], $pct[4], $s[0], $s[-1])
    return @{ med = $med; tmean = $tmean; s = $s }
}

$pipeA = "starship-throttle-a"
$pipeB = "starship-throttle-b"
$IDLE_MS = $IdleMs
$WARM = $Warm

Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PIPE = $pipeA; $env:STARSHIP_DAEMON_THROTTLE = "1"
$pA = Start-Process -FilePath $daemonExe -WorkingDirectory $repo -WindowStyle Hidden -PassThru
Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PIPE = $pipeB
$pB = Start-Process -FilePath $daemonExe -WorkingDirectory $repo -WindowStyle Hidden -PassThru
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue

function Wait-Pipe($name) {
    for ($i = 0; $i -lt 50; $i++) {
        try { $q = [System.IO.Pipes.NamedPipeClientStream]::new(".", $name); $q.Connect(200); $q.Dispose(); return } catch { Start-Sleep -Milliseconds 200 }
    }
    throw "pipe $name never appeared"
}
Wait-Pipe $pipeA
Wait-Pipe $pipeB

$pa = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeA); $pa.Connect(5000); $pa.ReadMode = [System.IO.Pipes.PipeTransmissionMode]::Message
$pb = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeB); $pb.Connect(5000); $pb.ReadMode = [System.IO.Pipes.PipeTransmissionMode]::Message
$buf = [ThFrame]::Build((Get-Location).ProviderPath)
$resp = [byte[]]::new(65536)

function Send-Raw($p) { $p.Write($buf, 0, $buf.Length); $p.Flush() }
function Recv-Raw($p) { $r = $p.Read($resp, 0, $resp.Length); if (-not [ThFrame]::Ok($resp, $r)) { throw "bad frame" } }

Write-Host "warming up ($WARM iters each)..."
for ($i = 0; $i -lt $WARM; $i++) { Send-Raw $pa; Recv-Raw $pa; Send-Raw $pb; Recv-Raw $pb }

$sA = New-Object System.Collections.Generic.List[double]
$sB = New-Object System.Collections.Generic.List[double]
Write-Host "measuring ($N iters each, ${IDLE_MS}ms idle, interleaved A,B)..."
for ($i = 0; $i -lt $N; $i++) {
    Start-Sleep -Milliseconds $IDLE_MS
    $t0 = QPC-Us; Send-Raw $pa; Recv-Raw $pa; $t1 = QPC-Us
    $sA.Add($t1 - $t0)
    Start-Sleep -Milliseconds $IDLE_MS
    $t0 = QPC-Us; Send-Raw $pb; Recv-Raw $pb; $t1 = QPC-Us
    $sB.Add($t1 - $t0)
    if (($i + 1) % 25 -eq 0) { Write-Host ("  {0}/{1}" -f ($i + 1), $N) }
}

$pa.Dispose(); $pb.Dispose()
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Write-Host "`n=== cold-call latency (us) ==="
$ra = Stats ($sA.ToArray()) "throttle-ON (=1)"
$rb = Stats ($sB.ToArray()) "throttle-OFF"
$p = Invoke-MannWhitneyU ($sA.ToArray()) ($sB.ToArray())
Write-Host ("`nMann-Whitney U p-value: {0:F4}  (n1=n2={1})" -f $p, $N)
if ($p -lt 0.05) { Write-Host "=> significant difference (p<0.05)" } else { Write-Host "=> no significant difference (p>=0.05)" }
$ratio = $ra.med / $rb.med
Write-Host ("median ON/OFF ratio: {0:F2}x" -f $ratio)
Write-Host "DONE"