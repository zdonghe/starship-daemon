param([string]$Pristine, [string]$OutBase)
$ErrorActionPreference = "Stop"
if (-not $Pristine) { $Pristine = "$PSScriptRoot\module_head.psm1" }
if (-not $OutBase) { $OutBase = "$PSScriptRoot\ab" }

function Insert-Before([string]$Text, [string]$Needle, [string]$Insert) {
    $i = $Text.IndexOf($Needle)
    if ($i -lt 0) { throw "needle not found: $Needle" }
    return $Text.Substring(0, $i) + $Insert + $Text.Substring($i)
}

function Insert-After([string]$Text, [string]$Needle, [string]$Insert) {
    $i = $Text.IndexOf($Needle)
    if ($i -lt 0) { throw "needle not found: $Needle" }
    $j = $i + $Needle.Length
    return $Text.Substring(0, $j) + $Insert + $Text.Substring($j)
}

function Replace-One([string]$Text, [string]$Old, [string]$New) {
    $i = $Text.IndexOf($Old)
    if ($i -lt 0) { throw "old not found: $Old" }
    return $Text.Substring(0, $i) + $New + $Text.Substring($i + $Old.Length)
}

function Save([string]$Name, [string]$Src) {
    $dir = "$OutBase\$Name"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $dst = "$dir\starship-daemon.psm1"
    [System.IO.File]::WriteAllText($dst, $Src, [System.Text.UTF8Encoding]::new($false))
    $hits = Select-String -Path $dst -Pattern "LogicSw|LastLogicUs" | Measure-Object
    $sha = (Get-FileHash $dst -Algorithm SHA256).Hash.Substring(0,12)
    Write-Host ("{0,-18} bracket={1} sha={2}" -f $Name, $hits.Count, $sha)
}

$src = Get-Content -LiteralPath $Pristine -Raw
if (-not $src) { throw "cannot read pristine $Pristine" }
$src = $src -replace "`r`n", "`n"

$hoistMarker = "`$script:DaemonDownRetryAt = [DateTime]::MinValue"

# The shipped corrected set: A1 = hoist ONLY the pipe name (cache byte stays a
# per-call $env read), plus A3 (LastBuildKey/LastBuildBuf frame cache). A5 and
# the cache-byte/A2 hoists were rejected for correctness/perf - keep pristine.
$c = $src
$c = Insert-After $c $hoistMarker "`n`n`$script:DaemonPipeName = if (`$env:STARSHIP_DAEMON_PIPE) { `$env:STARSHIP_DAEMON_PIPE } else { ""starship-daemon"" }"
$c = Replace-One $c "`$pipeName = if (`$env:STARSHIP_DAEMON_PIPE) { `$env:STARSHIP_DAEMON_PIPE } else { ""starship-daemon"" }" "`$pipeName = `$script:DaemonPipeName"
$c = Insert-After $c $hoistMarker "`n`n`$script:LastBuildKey = `$null`n`$script:LastBuildBuf = `$null"
$oldBuild = "`$buf = [StarshipFrame]::Build(`$cwd, `$ExitCode, `$keymap, `$Width, `$config, [byte]`$disableCache)"
$newBuild = "`$buildKey = ""`$cwd|`$ExitCode|`$keymap|`$Width|`$config|`$disableCache""; if (`$buildKey -ne `$script:LastBuildKey) { `$script:LastBuildKey = `$buildKey; `$script:LastBuildBuf = [StarshipFrame]::Build(`$cwd, `$ExitCode, `$keymap, `$Width, `$config, [byte]`$disableCache) }; `$buf = `$script:LastBuildBuf"
$c = Replace-One $c $oldBuild $newBuild

# Logic bracket: start before cwd, stop before pipe.Write
$sw  = "`$script:LogicSw = [System.Diagnostics.Stopwatch]::StartNew()`n`n"
$stop = "`$script:LastLogicUs = `$script:LogicSw.Elapsed.TotalMicroseconds`n            `$script:LogicSw.Stop()`n`n"
$li = Insert-Before $c "        `$cwd = `$PWD.ProviderPath" $sw
$li = Insert-Before $li "            `$pipe.Write(`$buf, 0, `$buf.Length)" $stop
Save "corrected_internal" $li

# Prelude bracket: start after origLastExitCode, stop before result=$null
$BOPEN = '    $script:PreludeSw = [System.Diagnostics.Stopwatch]::StartNew()'
$BCLOSE = '    $script:LastPreludeUs = $script:PreludeSw.Elapsed.TotalMicroseconds; $script:PreludeSw.Stop()'
$pi = $c.Replace("    `$origLastExitCode = `$global:LASTEXITCODE", "    `$origLastExitCode = `$global:LASTEXITCODE`n$BOPEN")
if ($pi -eq $c) { throw "prelude open needle not found" }
$pi = $pi.Replace("`n    `$result = `$null", "`n$BCLOSE`n    `$result = `$null")
if ($pi -eq $c) { throw "prelude close needle not found" }
Save "corrected_prelude" $pi

Write-Host "DONE"