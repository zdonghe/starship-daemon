param([int]$BudgetSeconds = 10, [int]$WarmSamples = 30)
$ErrorActionPreference = "Stop"
$repo = (Split-Path $PSScriptRoot -Parent)
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
    public static int Len(byte[] b, int r) { if (r < 4) return 0; int l = (int)(b[0]|(b[1]<<8)|(b[2]<<16)|(b[3]<<24)); return (l>0 && l<=65531 && r>=4+l) ? l : 0; }
}
"@
Add-Type -TypeDefinition $FrameSrc

$pipeName = "starship-bd-test"
Remove-Item Env:STARSHIP_DAEMON_THROTTLE -ErrorAction SilentlyContinue
$env:STARSHIP_DAEMON_PIPE = $pipeName   # throttle OFF (opt-out = default)
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400
$p = Start-Process -FilePath $daemonExe -WorkingDirectory $repo -WindowStyle Hidden -PassThru
for ($i=0;$i-lt 50;$i++){ try{$q=[System.IO.Pipes.NamedPipeClientStream]::new(".",$pipeName);$q.Connect(200);$q.Dispose();break}catch{Start-Sleep -Milliseconds 200}}
$c = [System.IO.Pipes.NamedPipeClientStream]::new(".", $pipeName); $c.Connect(5000); $c.ReadMode = [System.IO.Pipes.PipeTransmissionMode]::Message
$buf = [ThFrame]::Build((Get-Location).ProviderPath); $resp = [byte[]]::new(65536)

# warmup
for ($i=0;$i-lt 20;$i++){ $c.Write($buf,0,$buf.Length);$c.Flush(); $r=$c.Read($resp,0,$resp.Length) }

# COLD phase: idle 3s between requests until budget elapsed
Write-Host ("COLD phase: measuring until {0}s elapsed (idle 3s/sample), throttle OFF (opt-out)..." -f $BudgetSeconds)
$costop = [System.Diagnostics.Stopwatch]::StartNew()
$cWrite = New-Object System.Collections.Generic.List[double]
$cRead  = New-Object System.Collections.Generic.List[double]
$cTotal = New-Object System.Collections.Generic.List[double]
$n = 0
while ($costop.Elapsed.TotalSeconds -lt $BudgetSeconds) {
    Start-Sleep -Milliseconds 3000
    $t0 = QPC-Us
    $c.Write($buf,0,$buf.Length); $c.Flush(); $tw = QPC-Us
    $rn = $c.Read($resp,0,$resp.Length); $tr = QPC-Us
    if ([ThFrame]::Len($resp,$rn) -eq 0) { throw "bad frame" }
    $cWrite.Add($tw-$t0); $cRead.Add($tr-$tw); $cTotal.Add($tr-$t0)
    $n++
}
Write-Host ("  cold samples: {0}" -f $n)

# WARM phase: back-to-back
Write-Host ("WARM phase: {0} back-to-back requests..." -f $WarmSamples)
$wWrite = New-Object System.Collections.Generic.List[double]
$wRead  = New-Object System.Collections.Generic.List[double]
$wTotal = New-Object System.Collections.Generic.List[double]
for ($i=0;$i-lt $WarmSamples;$i++){
    $t0 = QPC-Us
    $c.Write($buf,0,$buf.Length); $c.Flush(); $tw = QPC-Us
    $rn = $c.Read($resp,0,$resp.Length); $tr = QPC-Us
    $wWrite.Add($tw-$t0); $wRead.Add($tr-$tw); $wTotal.Add($tr-$t0)
}
$c.Dispose()
Get-Process -Name starship-daemon -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-Item Env:STARSHIP_DAEMON_PIPE -ErrorAction SilentlyContinue

# throttle OFF = opt-out = throttle disabled
function Med([double[]]$a){ $s=$a|Sort-Object; return $s[[int]($s.Count/2)] }
function Trim([double[]]$a){ $s=$a|Sort-Object; $t=[int]($s.Count*0.10); return ($s[$t..($s.Count-$t-1)]|Measure-Object -Average).Average }

Write-Host "`n=== COLD (idle 3s) ==="
$cm=[math]::Round((Med $cWrite));$ct=[math]::Round((Trim $cWrite));$rm=[math]::Round((Med $cRead));$rt=[math]::Round((Trim $cRead));$tm=[math]::Round((Med $cTotal));$tt=[math]::Round((Trim $cTotal))
Write-Host ("  write : med={0,6} us   trim10={1,6} us   (share of total med: {2:P0})" -f $cm,$ct,($cm/$tm))
Write-Host ("  read  : med={0,6} us   trim10={1,6} us   (share of total med: {2:P0})" -f $rm,$rt,($rm/$tm))
Write-Host ("  total : med={0,6} us   trim10={1,6} us" -f $tm,$tt)

Write-Host "`n=== WARM (back-to-back) ==="
$wm=[math]::Round((Med $wWrite));$wr=[math]::Round((Med $wRead));$wt=[math]::Round((Med $wTotal))
Write-Host ("  write : med={0,6} us" -f $wm)
Write-Host ("  read  : med={0,6} us" -f $wr)
Write-Host ("  total : med={0,6} us" -f $wt)

Write-Host "`n=== COLD-OVER-WARM overhead (what idle adds) ==="
Write-Host ("  write : +{0,6} us  (warm {1} -> cold {2})" -f ($cm-$wm),$wm,$cm)
Write-Host ("  read  : +{0,6} us  (warm {1} -> cold {2})" -f ($rm-$wr),$wr,$rm)
Write-Host ("  total : +{0,6} us" -f ($tm-$wt))
Write-Host "DONE"