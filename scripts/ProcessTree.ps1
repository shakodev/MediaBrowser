# Process identity is PID + creation time, including for ancestry checks.
function Test-ProcessIdentity($Process, $Record) {
    return $null -ne $Process -and [Math]::Abs(($Process.CreationDate.ToUniversalTime() - $Record.Created).TotalSeconds) -lt 0.001
}

function Get-TrackedProcesses($Processes, [hashtable]$Known, [datetime]$ObservedAt) {
    $byId = @{}
    foreach ($proc in $Processes) { $byId[[uint32]$proc.ProcessId] = $proc }
    foreach ($id in @($Known.Keys)) {
        if (!(Test-ProcessIdentity $byId[$id] $Known[$id]) -and $null -eq $Known[$id].EndedAt) {
            $Known[$id].EndedAt = $ObservedAt
        }
    }
    do {
        $added = $false
        foreach ($proc in $Processes) {
            $id = [uint32]$proc.ProcessId
            $parentId = [uint32]$proc.ParentProcessId
            if ($Known.ContainsKey($id) -and (Test-ProcessIdentity $proc $Known[$id])) { continue }
            if (!$Known.ContainsKey($parentId)) { continue }
            $parent = $Known[$parentId]
            $created = $proc.CreationDate.ToUniversalTime()
            if ($created -lt $parent.Created) { continue }
            if ($byId.ContainsKey($parentId)) {
                if (!(Test-ProcessIdentity $byId[$parentId] $parent)) { continue }
            } elseif ($null -ne $parent.EndedAt -and $created -gt $parent.EndedAt) {
                continue
            }
            $Known[$id] = @{ Created = $created; EndedAt = $null }
            $added = $true
        }
    } while ($added)
    foreach ($proc in $Processes) {
        $id = [uint32]$proc.ProcessId
        if ($Known.ContainsKey($id) -and (Test-ProcessIdentity $proc $Known[$id])) { $proc }
    }
}
