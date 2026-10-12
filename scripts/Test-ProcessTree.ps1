$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/ProcessTree.ps1"
$start = ([datetime]'2026-10-12T12:00:00Z').ToUniversalTime()
function Proc([uint32]$Id, [uint32]$Parent, [int]$Age) {
    [pscustomobject]@{ProcessId=$Id;ParentProcessId=$Parent;CreationDate=$start.AddSeconds($Age)}
}
function Assert-Ids($Actual, $Expected) {
    $ids = (@($Actual | ForEach-Object ProcessId | Sort-Object) -join ',')
    if ($ids -ne $Expected) { throw "Expected $Expected, got $ids" }
}
$known = @{}
$known[[uint32]1] = @{Created=$start;EndedAt=$null}
$root = Proc 1 0 0
$child = Proc 2 1 1
$grandchild = Proc 3 2 2
# Unordered input still discovers recursive children, excluding unrelated apps.
Assert-Ids (Get-TrackedProcesses @($grandchild,(Proc 9 0 0),$child,$root) $known $start.AddSeconds(3)) '1,2,3'
# A known grandchild stays tracked after its parent exits.
Assert-Ids (Get-TrackedProcesses @($root,$grandchild) $known $start.AddSeconds(4)) '1,3'
# Reused parent PID must not admit a new unrelated process subtree.
Assert-Ids (Get-TrackedProcesses @($root,$grandchild,(Proc 2 9 5),(Proc 4 2 6)) $known $start.AddSeconds(7)) '1,3'
# Even if that unrelated parent has already exited, its later children are excluded.
Assert-Ids (Get-TrackedProcesses @($root,$grandchild,(Proc 4 2 6)) $known $start.AddSeconds(8)) '1,3'
# A genuinely new descendant can reuse a formerly tracked PID.
Assert-Ids (Get-TrackedProcesses @($root,(Proc 2 1 9),(Proc 4 2 10)) $known $start.AddSeconds(11)) '1,2,4'
Write-Host 'Process-tree identity, orphan and PID-reuse regression checks passed.'
