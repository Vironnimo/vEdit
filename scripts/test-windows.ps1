[CmdletBinding()]
param([string]$ExecutableDirectory)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $ExecutableDirectory) { $ExecutableDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'target/debug' }
$executable = (Resolve-Path -LiteralPath (Join-Path $ExecutableDirectory 'vedit.exe')).Path
$cli = (Resolve-Path -LiteralPath (Join-Path $ExecutableDirectory 'vedit-cli.exe')).Path
$testId = [Guid]::NewGuid().ToString('N')
$fixture = Join-Path ([IO.Path]::GetTempPath()) "vedit-window-tests/$testId"
$profile = Join-Path $fixture 'profile'
$began = Get-Date
$processes = [Collections.Generic.List[object]]::new()
$checks = 0
$previousProfile = $env:VEDIT_PROFILE_DIR
$previousControl = $env:VEDIT_CONTROL_DIR
$previousLegacyControl = $env:MORROW_CONTROL_DIR
function Assert($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
function Invoke-EditorCli {
    $json = & $script:cli @args
    $response = $json | ConvertFrom-Json
    if (-not $response.ok) { throw ($response.error | ConvertTo-Json -Compress) }
    $response.result
}
function Wait-For([scriptblock]$Check, [string]$Description) {
    $deadline = (Get-Date).AddSeconds(25)
    do {
        if (& $Check) { return }
        Start-Sleep -Milliseconds 150
    } while ((Get-Date) -lt $deadline)
    throw "Timed out: $Description"
}
function Launch([string]$Arguments) {
    $process = Start-Process -FilePath $executable -ArgumentList $Arguments -WorkingDirectory $fixture -WindowStyle Hidden -PassThru
    $processes.Add($process)
    $process
}
function Ready([string]$Id) {
    $response = & $cli --window $Id status | ConvertFrom-Json
    $response.ok -and @($response.result.tabs.items | Where-Object loading).Count -eq 0
}
function Stop-Fixture {
    # Endpoint IDs come exclusively from the private fixture. Also require the
    # expected executable and creation time before stopping an owned test process.
    if (Test-Path -LiteralPath $profile) {
        Get-ChildItem -LiteralPath $profile -Recurse -Filter control.json | ForEach-Object {
            $endpoint = Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json
            $process = Get-Process -Id $endpoint.pid -ErrorAction SilentlyContinue
            if ($process -and $process.Path -eq $executable -and $process.StartTime -ge $began) {
                Stop-Process -Id $process.Id
                $process.WaitForExit(5000) | Out-Null
            }
        }
    }
    foreach ($process in $processes) {
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id
            $process.WaitForExit(5000) | Out-Null
        }
    }
}
try {
    New-Item -ItemType Directory -Path (Join-Path $fixture 'project'), (Join-Path $fixture 'outside') -Force | Out-Null
    $env:VEDIT_PROFILE_DIR = $profile
    Remove-Item Env:VEDIT_CONTROL_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:MORROW_CONTROL_DIR -ErrorAction SilentlyContinue
    $project = Join-Path $fixture 'project'
    $inside = Join-Path $project 'inside.txt'
    $outside = Join-Path $fixture ('outside/space ' + [char]0x6771 + [char]0x4eac + '.txt')
    $second = Join-Path $fixture 'outside/second.txt'
    Set-Content -LiteralPath $inside -Value 'project text'
    Set-Content -LiteralPath $outside -Value 'loose text'
    Set-Content -LiteralPath $second -Value 'second text'
    Launch ('--reuse-window -- "' + $outside + '" "' + $project + '" "' + $inside + '"') | Out-Null
    Wait-For { Ready main } 'main window startup'
    $status = Invoke-EditorCli --window main status
    Assert ($status.recovery_directory.StartsWith($profile)) 'Test is not using its isolated recovery profile.'
    Assert ($status.tabs.total -eq 2) 'Mixed startup arguments lost files.'
    Assert ((@($status.tabs.items | Where-Object path -eq $outside))[0].workspace_kind -eq 'other_files') 'Loose startup file did not enter Other files.'
    Assert ((@($status.tabs.items | Where-Object path -eq $inside))[0].workspace_kind -eq 'project') 'Project startup file did not join its folder project.'
    $forwarder = Launch ('-- "' + $second + '"')
    Assert ($forwarder.WaitForExit(15000)) 'File launch failed to forward to the existing window.'
    Wait-For { (Invoke-EditorCli --window main tabs).total -eq 3 } 'forwarded file'
    Assert ((Invoke-EditorCli windows).windows.Count -eq 1) 'Default file opening created another window.'
    # Simultaneous new-window requests must allocate different profiles.
    Launch ('--new-window -- "' + $outside + '"') | Out-Null
    Launch ('--new-window -- "' + $second + '"') | Out-Null
    Wait-For { (Ready w1) -and (Ready w2) } 'two independent windows'
    Assert ((Invoke-EditorCli windows).windows.Count -eq 3) 'New windows share a single-instance lock.'
    foreach ($id in @('w1', 'w2')) {
        $state = Invoke-EditorCli --window $id status
        Assert ($state.tabs.total -eq 1 -and $state.workspaces.Count -eq 0) 'New window inherited unrelated documents.'
        Assert ($state.recovery_directory.EndsWith("windows\$id")) 'New window does not have independent storage.'
    }
    $ambiguous = & $cli status | ConvertFrom-Json
    Assert (-not $ambiguous.ok -and $ambiguous.error.message.Contains('--window ID')) 'Ambiguous CLI request was not rejected.'
    $unknown = & $cli --window w999 open $outside | ConvertFrom-Json
    Assert (-not $unknown.ok) 'Unknown window unexpectedly started.'
    $patch = Join-Path $fixture 'settings.json'
    Set-Content -LiteralPath $patch -Value '{"external_open_new_window":true}' -Encoding UTF8
    Invoke-EditorCli --window main settings $patch | Out-Null
    Wait-For { (Test-Path -LiteralPath (Join-Path $profile 'recovery/external-open.json')) -and (Get-Content -LiteralPath (Join-Path $profile 'recovery/external-open.json') -Raw).Trim() -eq 'true' } 'persisted external-open preference'
    Launch ('-- "' + $outside + '"') | Out-Null
    Wait-For { Ready w3 } 'preference-created window'
    Assert ((Invoke-EditorCli windows).windows.Count -eq 4) 'New-window preference was ignored.'
    Invoke-EditorCli --window w1 open $inside | Out-Null
    Assert ((Invoke-EditorCli windows).windows.Count -eq 4) 'Agent open followed the external-launch preference.'
    $otherStamp = Join-Path $profile 'recovery/windows/w2/last-active'
    $oldOtherFocus = if (Test-Path -LiteralPath $otherStamp) { Get-Content -LiteralPath $otherStamp -Raw } else { '' }
    Invoke-EditorCli --window w2 raise | Out-Null
    Wait-For { (Test-Path -LiteralPath $otherStamp) -and (Get-Content -LiteralPath $otherStamp -Raw) -ne $oldOtherFocus } 'other window focus timestamp'
    $focusedStamp = Join-Path $profile 'recovery/windows/w1/last-active'
    $oldFocus = if (Test-Path -LiteralPath $focusedStamp) { Get-Content -LiteralPath $focusedStamp -Raw } else { '' }
    Invoke-EditorCli --window w1 raise | Out-Null
    Wait-For { (Test-Path -LiteralPath $focusedStamp) -and (Get-Content -LiteralPath $focusedStamp -Raw) -ne $oldFocus } 'focus timestamp'
    Wait-For { (Invoke-EditorCli --window w1 settings).settings.external_open_new_window } 'shared preference in an already open window'
    Assert ((Invoke-EditorCli --window w1 settings).settings.external_open_new_window) 'Focused window shows a stale external-open preference.'
    $focusedFile = Join-Path $fixture 'outside/focused.txt'
    Set-Content -LiteralPath $focusedFile -Value 'reuse the last focused window'
    $forwarder = Launch ('--reuse-window -- "' + $focusedFile + '"')
    Assert ($forwarder.WaitForExit(15000)) 'Explicit reuse did not override the preference.'
    Assert (@((Invoke-EditorCli --window w1 tabs).items | Where-Object path -eq $focusedFile).Count -eq 1) 'Reuse did not choose the last active window.'
    $draft = Join-Path $fixture 'draft.txt'
    Set-Content -LiteralPath $draft -Value 'recover this separate draft' -Encoding UTF8
    Invoke-EditorCli --window w1 new 'Separate draft' --file $draft | Out-Null
    Wait-For { Test-Path -LiteralPath (Join-Path $profile 'recovery/windows/w1/session.json') } 'secondary recovery manifest'
    Wait-For { (Get-Content -LiteralPath (Join-Path $profile 'recovery/windows/w1/session.json') -Raw).Contains('Separate draft') } 'secondary draft persistence'
    Wait-For {
        foreach ($id in @('main', 'w1', 'w2', 'w3')) {
            $relative = if ($id -eq 'main') { 'recovery/session.json' } else { "recovery/windows/$id/session.json" }
            $sessionFile = Join-Path $profile $relative
            if (-not (Test-Path -LiteralPath $sessionFile)) { return $false }
            $session = Get-Content -LiteralPath $sessionFile -Raw | ConvertFrom-Json
            if (@($session.projects | ForEach-Object { $_.tabs }).Count -eq 0) { return $false }
        }
        return $true
    } 'all window sessions persisted before the simulated crash'
    Stop-Fixture
    Launch '--reuse-window' | Out-Null
    Wait-For { (Ready main) -and (Ready w1) -and (Ready w2) -and (Ready w3) } 'interrupted windows restored'
    Assert ((Invoke-EditorCli --window main tabs).total -eq 3) 'Main session was overwritten by another window.'
    $recovered = (Invoke-EditorCli --window w1 tabs).items | Where-Object title -eq 'Separate draft'
    Assert ($null -ne $recovered -and $recovered.dirty) 'Separate unsaved draft was not restored.'
    $text = Invoke-EditorCli --window w1 read $recovered.id
    Assert (($text | ConvertTo-Json -Depth 10).Contains('recover this separate draft')) 'Recovered draft text differs.'
    $closedWindow = Invoke-EditorCli --window w2 status
    $closedProcess = Get-Process -Id $closedWindow.pid
    Assert ($closedProcess.CloseMainWindow()) 'Clean fixture window could not close normally.'
    Assert ($closedProcess.WaitForExit(10000)) 'Clean fixture window did not finish closing.'
    Assert (-not (Test-Path -LiteralPath (Join-Path $profile 'recovery/windows/w2/window-open'))) 'Normal close left an interrupted-window marker.'
    Assert ('w2' -in (Invoke-EditorCli windows).saved_windows) 'Closed window is not discoverable for restoration.'
    Launch '--window w2' | Out-Null
    Wait-For { Ready w2 } 'explicitly restored closed window'
    Assert ((Invoke-EditorCli --window w2 tabs).total -eq 1) 'Explicit restoration lost the closed window session.'
    Write-Output "$checks executable window checks passed with a private profile."
} catch {
    foreach ($id in @('main', 'w1', 'w2', 'w3')) {
        $response = & $cli --window $id status | ConvertFrom-Json
        Write-Output ("Window {0}: ok={1}, error={2}" -f $id, $response.ok, ($response.error | ConvertTo-Json -Compress))
    }
    throw
} finally {
    Stop-Fixture
    $env:VEDIT_PROFILE_DIR = $previousProfile
    $env:VEDIT_CONTROL_DIR = $previousControl
    $env:MORROW_CONTROL_DIR = $previousLegacyControl
    $resolved = [IO.Path]::GetFullPath($fixture)
    $prefix = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'vedit-window-tests')) + '\'
    if ($resolved.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -eq $testId) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
