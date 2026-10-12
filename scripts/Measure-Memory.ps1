param(
    [string]$Executable = "$PSScriptRoot/../src-tauri/target/release/media-browser.exe",
    [string]$OutputDirectory = "$PSScriptRoot/../.verification/memory-$(Get-Date -Format yyyyMMdd-HHmmss)",
    [string]$VideoUrl = 'https://www.youtube.com/watch?v=aqz-KE-bpKQ',
    [ValidateRange(5,120)][int]$SecondsPerPhase = 20,
    [string]$ResumeCheckpoint
)
$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Executable).Path
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath (Join-Path $outputPath 'phase.json')) { throw 'Use a fresh output directory for each run.' }
New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
if ($ResumeCheckpoint) {
    $stateDirectory = Join-Path $outputPath 'state'
    New-Item -ItemType Directory -Force -Path $stateDirectory | Out-Null
    Copy-Item -LiteralPath $ResumeCheckpoint -Destination (Join-Path $stateDirectory 'playback-state.json')
    $env:MEDIABROWSER_BENCH_RESUME = '1'
}
$env:MEDIABROWSER_BENCH_DIR = $outputPath
$env:MEDIABROWSER_BENCH_URL = $VideoUrl
$env:MEDIABROWSER_BENCH_SECONDS = "$SecondsPerPhase"
$app = Start-Process -FilePath $exePath -PassThru -WindowStyle Hidden
Remove-Item Env:MEDIABROWSER_BENCH_DIR, Env:MEDIABROWSER_BENCH_URL, Env:MEDIABROWSER_BENCH_SECONDS
Remove-Item Env:MEDIABROWSER_BENCH_RESUME -ErrorAction SilentlyContinue
$known = @{}
$known[[uint32]$app.Id] = $app.StartTime.ToUniversalTime()
$samples = [Collections.Generic.List[object]]::new()
$processRows = [Collections.Generic.List[object]]::new()
$clock = [Diagnostics.Stopwatch]::StartNew()
$lastPhase = ''
$rootExited = $null
while ($clock.Elapsed.TotalSeconds -lt 600) {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate, PrivatePageCount, ThreadCount)
    # Remember discovered descendants after their parents exit, and reject PID
    # reuse by checking creation times. Never sum unrelated WebView2 instances.
    do {
        $added = $false
        foreach ($proc in $all) {
            $id = [uint32]$proc.ProcessId
            $parent = [uint32]$proc.ParentProcessId
            if (!$known.ContainsKey($id) -and $known.ContainsKey($parent) -and $proc.CreationDate.ToUniversalTime() -ge $known[$parent]) {
                $known[$id] = $proc.CreationDate.ToUniversalTime()
                $added = $true
            }
        }
    } while ($added)
    $tree = @($all | Where-Object { $known.ContainsKey([uint32]$_.ProcessId) -and [Math]::Abs(($_.CreationDate.ToUniversalTime() - $known[[uint32]$_.ProcessId]).TotalSeconds) -lt 0.1 })
    $phase = 'launch'
    $phaseFile = Join-Path $outputPath 'phase.json'
    if (Test-Path -LiteralPath $phaseFile) { $phase = (Get-Content -Raw -LiteralPath $phaseFile | ConvertFrom-Json).phase }
    if ($phase -ne $lastPhase) { Write-Host $phase; $lastPhase = $phase }
    $elapsed = [Math]::Round($clock.Elapsed.TotalSeconds, 3)
    $total = [long]0
    foreach ($proc in $tree) {
        $bytes = [long]$proc.PrivatePageCount
        $total += $bytes
        $processRows.Add([pscustomobject]@{ Seconds=$elapsed; Phase=$phase; Pid=$proc.ProcessId; ParentPid=$proc.ParentProcessId; Name=$proc.Name; PrivateBytes=$bytes; Threads=$proc.ThreadCount })
    }
    $samples.Add([pscustomobject]@{ Seconds=$elapsed; Phase=$phase; Processes=$tree.Count; PrivateBytes=$total; PrivateMiB=[Math]::Round($total/1MB,3) })
    $app.Refresh()
    if ($app.HasExited) {
        if ($null -eq $rootExited) { $rootExited = $clock.Elapsed.TotalSeconds }
        if ($tree.Count -eq 0 -or $clock.Elapsed.TotalSeconds - $rootExited -gt 15) { break }
    }
    Start-Sleep -Milliseconds 500
}
$samples | Export-Csv -NoTypeInformation -LiteralPath (Join-Path $outputPath 'samples.csv')
$processRows | Export-Csv -NoTypeInformation -LiteralPath (Join-Path $outputPath 'processes.csv')
$summary = @($samples | Group-Object Phase | ForEach-Object {
    $values = @($_.Group.PrivateMiB | Sort-Object)
    $middle = [int][Math]::Floor($values.Count/2)
    $median = if ($values.Count % 2) { $values[$middle] } else { ($values[$middle - 1] + $values[$middle]) / 2 }
    [pscustomobject]@{ Phase=$_.Name; Samples=$values.Count; MedianPrivateMiB=[Math]::Round($median, 3); MinPrivateMiB=$values[0]; MaxPrivateMiB=$values[-1]; MaxProcesses=($_.Group.Processes | Measure-Object -Maximum).Maximum }
})
$summary | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $outputPath 'summary.json')
$summary | Format-Table
$result = [pscustomobject]@{ Executable=$exePath; Video=$VideoUrl; RootPid=$app.Id; Exited=$app.HasExited; RemainingDescendants=@($tree | Select-Object ProcessId, Name); DurationSeconds=$clock.Elapsed.TotalSeconds; Metric='Sum of Win32_Process.PrivatePageCount (private committed bytes), including descendants'; OS=[Environment]::OSVersion.VersionString }
$result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $outputPath 'run.json')
if (!$app.HasExited -or $tree.Count -gt 0 -or $lastPhase -ne 'complete') { throw "Benchmark incomplete or process tree did not terminate; inspect $outputPath" }
Write-Host "Results saved to $outputPath"
