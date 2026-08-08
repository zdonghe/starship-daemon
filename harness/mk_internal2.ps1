param([string]$Pristine, [string]$OutBase)
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
    Set-Content -LiteralPath $dst -Value $Src -Encoding utf8
    $hits = Select-String -Path $dst -Pattern "LogicSw|LastLogicUs" | Measure-Object
    $sha = (Get-FileHash $dst -Algorithm SHA256).Hash.Substring(0,12)
    Write-Host ("{0,-14} bracket={1} sha={2}" -f $Name, $hits.Count, $sha)
}

function Add-Bracket([string]$Src) {
    $sw  = "`$script:LogicSw = [System.Diagnostics.Stopwatch]::StartNew()`n`n"
    $stop = "`$script:LastLogicUs = `$script:LogicSw.Elapsed.TotalMicroseconds`n            `$script:LogicSw.Stop()`n`n"
    $src = Insert-Before $Src "        `$cwd = `$PWD.ProviderPath" $sw
    $src = Insert-Before $src "            `$pipe.Write(`$buf, 0, `$buf.Length)" $stop
    return $src
}

$src = Get-Content -LiteralPath $Pristine -Raw
if (-not $src) { throw "cannot read pristine $Pristine" }
$src = $src -replace "`r`n", "`n"

$hoistMarker = "`$script:DaemonDownRetryAt = [DateTime]::MinValue"

# HEAD + bracket (no logic edits)
Save "head_internal" (Add-Bracket $src)

# A1: hoist pipe name + cache byte to script vars; use them in Get-StarshipPrompt
$a1 = $src
$a1 = Insert-After $a1 $hoistMarker "`n`n`$script:DaemonPipeName = if (`$env:STARSHIP_DAEMON_PIPE) { `$env:STARSHIP_DAEMON_PIPE } else { ""starship-daemon"" }`n`$script:DisableCacheByte = [byte]0; if (`$env:STARSHIP_DAEMON_CACHE -eq ""0"") { `$script:DisableCacheByte = [byte]1 }"
$a1 = Replace-One $a1 "`$pipeName = if (`$env:STARSHIP_DAEMON_PIPE) { `$env:STARSHIP_DAEMON_PIPE } else { ""starship-daemon"" }" "`$pipeName = `$script:DaemonPipeName"
$a1 = Replace-One $a1 "`$disableCache = 0`n            if (`$env:STARSHIP_DAEMON_CACHE -eq ""0"")`n            { `$disableCache = 1 }" "`$disableCache = `$script:DisableCacheByte"
Save "a1_internal" (Add-Bracket $a1)

# A2: cache STARSHIP_CONFIG at import; currentCfg reads the cached var
$a2 = $src
$a2 = Insert-After $a2 $hoistMarker "`n`n`$script:CachedConfig = `$env:STARSHIP_CONFIG"
$a2 = Replace-One $a2 "`$currentCfg = `$env:STARSHIP_CONFIG" "`$currentCfg = `$script:CachedConfig"
Save "a2_internal" (Add-Bracket $a2)

# A3: build-key cache (LastBuildKey/LastBuildBuf)
$a3 = $src
$a3 = Insert-After $a3 $hoistMarker "`n`n`$script:LastBuildKey = `$null`n`$script:LastBuildBuf = `$null"
$oldBuild = "`$buf = [StarshipFrame]::Build(`$cwd, `$ExitCode, `$keymap, `$Width, `$config, [byte]`$disableCache)"
$newBuild = "`$buildKey = ""`$cwd|`$ExitCode|`$keymap|`$Width|`$config|`$disableCache""; if (`$buildKey -ne `$script:LastBuildKey) { `$script:LastBuildKey = `$buildKey; `$script:LastBuildBuf = [StarshipFrame]::Build(`$cwd, `$ExitCode, `$keymap, `$Width, `$config, [byte]`$disableCache) }; `$buf = `$script:LastBuildBuf"
$a3 = Replace-One $a3 $oldBuild $newBuild
Save "a3_internal" (Add-Bracket $a3)

# A5: ProviderPath -> PWD.Path for FileSystem (bracket first, then the edit)
$a5 = Add-Bracket $src
$a5 = Replace-One $a5 "`$cwd = `$PWD.ProviderPath" "`$cwd = if (`$PWD.Provider.Name -eq ""FileSystem"") { `$PWD.Path } else { `$PWD.ProviderPath }"
Save "a5_internal" $a5

Write-Host "done"