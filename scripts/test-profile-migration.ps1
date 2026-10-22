param([string]$ExecutableDirectory)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $ExecutableDirectory) { $ExecutableDirectory = Join-Path $PSScriptRoot '../target/debug' }
$executable = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory 'vedit.exe'))
$cli = [IO.Path]::GetFullPath((Join-Path $ExecutableDirectory 'vedit-cli.exe'))
$testId = [Guid]::NewGuid().ToString('N')
$fixture = Join-Path ([IO.Path]::GetTempPath()) "vedit-migration-tests/$testId"
$appdata = Join-Path $fixture 'appdata'
$profile = Join-Path $appdata 'vEdit'
$old = Join-Path $appdata 'Morrow/recovery'
$began = Get-Date
$processes = [Collections.Generic.List[Diagnostics.Process]]::new()
$environment = @{}
$checks = 0
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
function Assert($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
function Write-File([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function Invoke-EditorCli {
    $result = & $script:cli @args | ConvertFrom-Json
    if (-not $result.ok) { throw ($result.error | ConvertTo-Json -Compress) }
    $result.result
}
function Wait-Ready {
    $deadline = (Get-Date).AddSeconds(30)
    do {
        $ready = $true
        foreach ($id in @('main', 'w1')) {
            $result = & $cli --window $id status | ConvertFrom-Json
            if (-not $result.ok -or @($result.result.tabs.items | Where-Object loading).Count -ne 0) { $ready = $false }
        }
        if ($ready) { return }
        Start-Sleep -Milliseconds 150
    } while ((Get-Date) -lt $deadline)
    throw 'Migrated windows did not become ready.'
}
function Stop-Fixture {
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
        if (-not $process.HasExited) { Stop-Process -Id $process.Id; $process.WaitForExit(5000) | Out-Null }
    }
}
try {
    foreach ($key in @('APPDATA', 'VEDIT_PROFILE_DIR', 'VEDIT_CONTROL_DIR', 'MORROW_CONTROL_DIR')) {
        $environment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
        [Environment]::SetEnvironmentVariable($key, $null, 'Process')
    }
    $env:APPDATA = $appdata
    foreach ($id in @('main', 'w1')) {
        $directory = if ($id -eq 'main') { $old } else { Join-Path $old 'windows/w1' }
        $draft = Join-Path $directory 'draft.txt'
        Write-File $draft "unsaved $id draft $([char]0x6771)$([char]0x4eac)"
        $session = @{ autosave = $false; projects = @(@{
            kind = 'other_files'; root = ''; tabs = @(@{ title = "$id imported draft"; recovery_path = $draft; agent_unsaved = $true; cursor = 3 })
        }) }
        Write-File (Join-Path $directory 'session.json') ($session | ConvertTo-Json -Depth 10)
    }
    Write-File (Join-Path $old 'windows/w1/window-open') '4294967295'
    Write-File (Join-Path $appdata 'morrow-editor/data/app.ron') '{}'
    $hash = (Get-FileHash -LiteralPath (Join-Path $old 'session.json')).Hash
    $processes.Add((Start-Process -FilePath $executable -ArgumentList '--reuse-window' -WorkingDirectory $fixture -WindowStyle Hidden -PassThru))
    Wait-Ready
    Assert (-not (Test-Path -LiteralPath (Join-Path $appdata 'Morrow'))) 'Old profile directory is still active.'
    Assert (-not (Test-Path -LiteralPath (Join-Path $appdata 'morrow-editor'))) 'Old window-state directory is still active.'
    Assert ((Get-FileHash -LiteralPath (Join-Path $profile 'migration-backup/previous-profile/recovery/session.json')).Hash -eq $hash) 'Original session backup changed.'
    foreach ($id in @('main', 'w1')) {
        $status = Invoke-EditorCli --window $id status
        Assert ($status.recovery_directory.StartsWith($profile)) 'Window is not using the migrated profile.'
        Assert ($status.tabs.total -eq 1) 'Migrated draft is missing.'
        $tab = $status.tabs.items[0]
        Assert ($tab.dirty -and $null -eq $tab.error) 'Migrated draft is not safely recoverable.'
        $text = Invoke-EditorCli --window $id read $tab.id
        Assert (($text | ConvertTo-Json -Depth 10).Contains("unsaved $id draft $([char]0x6771)$([char]0x4eac)")) 'Migrated text differs.'
    }
    # An obsolete selector must not route even a read to the default profile.
    $env:MORROW_CONTROL_DIR = Join-Path $fixture 'previous-selector'
    $rejected = & $cli status | ConvertFrom-Json
    Assert (-not $rejected.ok -and $rejected.error.message.Contains('VEDIT_CONTROL_DIR')) 'Obsolete instance selector was silently ignored.'
    Remove-Item Env:MORROW_CONTROL_DIR
    Stop-Fixture
    $processes.Add((Start-Process -FilePath $executable -ArgumentList '--reuse-window' -WorkingDirectory $fixture -WindowStyle Hidden -PassThru))
    Wait-Ready
    Assert ((Invoke-EditorCli windows).windows.Count -eq 2) 'Second launch failed to restore independent windows.'
    Assert (-not (Test-Path -LiteralPath (Join-Path $appdata 'Morrow'))) 'Second launch recreated an old profile.'
    Write-Output "$checks executable migration checks passed with isolated APPDATA."
} finally {
    Stop-Fixture
    foreach ($key in $environment.Keys) { [Environment]::SetEnvironmentVariable($key, $environment[$key], 'Process') }
    $resolved = [IO.Path]::GetFullPath($fixture)
    $prefix = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'vedit-migration-tests')) + '\'
    if ($resolved.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -eq $testId) {
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
    }
}
