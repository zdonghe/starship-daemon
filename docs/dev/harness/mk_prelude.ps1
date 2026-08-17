$ErrorActionPreference = "Stop"
$base = "$PSScriptRoot\ab"
$headPath = "$PSScriptRoot\module_head.psm1"
$srcHash = (Get-FileHash $headPath).Hash.Substring(0,8)
if ($srcHash -ne "B70BC4E8") { throw "module_head.psm1 is NOT pristine HEAD (got $srcHash) - abort" }
Write-Host "base is pristine HEAD ($srcHash)"

$text = Get-Content $headPath -Raw
$text = $text -replace "`r`n", "`n"

$BOPEN = '    $script:PreludeSw = [System.Diagnostics.Stopwatch]::StartNew()'
$BCLOSE = '    $script:LastPreludeUs = $script:PreludeSw.Elapsed.TotalMicroseconds; $script:PreludeSw.Stop()'

# pristine + bracket
$sw = $text
$sw = $sw.Replace("    `$origLastExitCode = `$global:LASTEXITCODE", "    `$origLastExitCode = `$global:LASTEXITCODE`n$BOPEN")
$sw = $sw.Replace("`n    `$result = `$null", "`n$BCLOSE`n    `$result = `$null")

function Save($name, $t) {
    $dir = "$base\$name"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $t = $t -replace "`r", ""
    [System.IO.File]::WriteAllText("$dir\starship-daemon.psm1", $t, [System.Text.UTF8Encoding]::new($false))
    $h = (Get-FileHash "$dir\starship-daemon.psm1").Hash.Substring(0,8)
    $amarkers = (Select-String -Path "$dir\starship-daemon.psm1" -Pattern 'DaemonPipeName|DisableCacheByte|CachedConfig|LastBuildKey|LastBuildBuf|\$PWD.Provider.Name').Count
    $brackets = (Select-String -Path "$dir\starship-daemon.psm1" -Pattern 'PreludeSw').Count
    Write-Host ("{0,-12} sha={1} lines={2} A-markers={3} PreludeSw={4}" -f $name,$h,(Get-Content "$dir\starship-daemon.psm1").Count,$amarkers,$brackets)
    if ($amarkers -gt 0) { throw "A-markers present in $name - ABORT" }
}

Save "p_head" $sw

$b1 = $sw
$b1 = $b1.Replace("if (Test-Path function:Invoke-Starship-PreCommand)", "if (`$script:HasPreCommand)")
$b1 = $b1.Replace("    `$script:PreludeSw = [System.Diagnostics.Stopwatch]::StartNew()",
                  "    `$script:HasPreCommand = [bool](Get-Command Invoke-Starship-PreCommand -ErrorAction SilentlyContinue)`n$BOPEN")
Save "p_b1" $b1

$b2 = $sw
$b2 = $b2.Replace("`n    `$loc = `$executionContext.SessionState.Path.CurrentLocation`n", "`n")
$b2 = $b2.Replace('$result = "PS $($loc.ProviderPath)> "', 'if (-not $loc) { $loc = $executionContext.SessionState.Path.CurrentLocation }; $result = "PS $($loc.ProviderPath)> "')
Save "p_b2" $b2

$b3 = $sw
$oldGate = "    `$exitCode = 0`n    if (-not `$origDollarQuestion)`n    {`n        if (`$lastCmd = Get-History -Count 1)`n        {`n            `$lastCmdletError = try { `$global:error[0].InvocationInfo } catch { `$null }`n            if (`$null -ne `$lastCmdletError -and `$lastCmd.CommandLine -eq `$lastCmdletError.Line) { `$exitCode = 1 } else { `$exitCode = `$origLastExitCode }`n        }`n    }"
$newGate = "    `$exitCode = 0`n    if (-not `$origDollarQuestion)`n    {`n        if (`$lastCmd = Get-History -Count 1)`n        {`n            if (`$lastCmd.CommandLine -ne `$script:LastExitCmdLine -or -not `$script:LastExitChecked)`n            {`n                `$script:LastExitCmdLine = `$lastCmd.CommandLine`n                `$script:LastExitChecked = `$true`n                `$lastCmdletError = try { `$global:error[0].InvocationInfo } catch { `$null }`n                if (`$null -ne `$lastCmdletError -and `$lastCmd.CommandLine -eq `$lastCmdletError.Line) { `$script:LastExitCode = 1 } else { `$script:LastExitCode = `$origLastExitCode }`n            }`n            `$exitCode = `$script:LastExitCode`n        }`n    }"
if (-not $sw.Contains($oldGate)) { throw "B3 oldGate not found" }
$b3 = $b3.Replace($oldGate, $newGate)
Save "p_b3" $b3

Write-Host "DONE"