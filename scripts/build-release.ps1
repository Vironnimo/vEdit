$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = Split-Path -Parent $PSScriptRoot
$releaseDirectory = Join-Path $projectRoot 'dist/Morrow'
$preservedBinaries = [System.Collections.Generic.List[object]]::new()

function Preserve-RunningEditor([string]$BinaryPath) {
    $absoluteBinary = [System.IO.Path]::GetFullPath($BinaryPath)
    $workspacePrefix = [System.IO.Path]::GetFullPath($projectRoot).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $absoluteBinary.StartsWith($workspacePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'The release binary must remain inside the project workspace.'
    }
    if (-not (Test-Path -LiteralPath $absoluteBinary -PathType Leaf)) { return }
    try {
        $probe = [System.IO.File]::Open($absoluteBinary, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $probe.Dispose()
        return
    }
    catch [System.IO.IOException] {
        $runningEditor = Get-Process -Name 'morrow' -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and [System.IO.Path]::GetFullPath($_.Path) -eq $absoluteBinary } |
            Select-Object -First 1
        if (-not $runningEditor) { throw }
        $backup = Join-Path (Split-Path -Parent $absoluteBinary) "morrow-running-$($runningEditor.Id).exe"
        if (Test-Path -LiteralPath $backup) { throw "The running editor backup already exists: $backup" }
        Move-Item -LiteralPath $absoluteBinary -Destination $backup -ErrorAction Stop
        $preservedBinaries.Add([pscustomobject]@{ Original = $absoluteBinary; Backup = $backup })
        Write-Output "Preserved the running editor (PID $($runningEditor.Id)); its window and documents remain open."
    }
}

Push-Location $projectRoot
try {
    Preserve-RunningEditor (Join-Path $projectRoot 'target/release/morrow.exe')
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $executable = Join-Path $releaseDirectory 'morrow.exe'
    Preserve-RunningEditor $executable
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/morrow.exe') -Destination $executable -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/morrow-cli.exe') -Destination $releaseDirectory -Force
    & (Join-Path $PSScriptRoot 'package-language-tools.ps1') -ReleaseDirectory $releaseDirectory
    $packagedReadme = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'README.md')).Replace('(.agents/skills/morrow/SKILL.md)', '(skills/morrow/SKILL.md)')
    [System.IO.File]::WriteAllText((Join-Path $releaseDirectory 'README.md'), $packagedReadme, [System.Text.UTF8Encoding]::new($false))
    $previewDirectory = Join-Path $releaseDirectory 'tests/snapshots'
    New-Item -ItemType Directory -Path $previewDirectory -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'tests/snapshots') -File |
        Where-Object { $_.Name -match '^morrow_[a-z0-9_]+\.png$' } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $previewDirectory -Force }
    $skillDirectory = Join-Path $releaseDirectory 'skills/morrow'
    New-Item -ItemType Directory -Path $skillDirectory -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/morrow/SKILL.md') -Destination $skillDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/morrow/agents') -Destination $skillDirectory -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/morrow/references') -Destination $skillDirectory -Recurse -Force
    $documentationDirectory = Join-Path $releaseDirectory 'docs'
    New-Item -ItemType Directory -Path $documentationDirectory -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/agent-interface.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/user-review-0.4.9.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/code-review-0.4.10.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/usability-review-0.4.11.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/polyglot-keyboard-review-0.4.12.md') -Destination $documentationDirectory -Force
    $packagedPolish = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'docs/interaction-polish-0.4.13.md')).Replace('../vendor/winit/MORROW_PATCH.md', '../licenses/winit/MORROW_PATCH.md')
    [System.IO.File]::WriteAllText((Join-Path $documentationDirectory 'interaction-polish-0.4.13.md'), $packagedPolish, [System.Text.UTF8Encoding]::new($false))
    $fontLicenseDirectory = Join-Path $releaseDirectory 'assets/fonts'
    New-Item -ItemType Directory -Path $fontLicenseDirectory -Force | Out-Null
    foreach ($fontDocumentation in @('OFL.txt', 'README.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "assets/fonts/$fontDocumentation") -Destination $fontLicenseDirectory -Force
    }
    $syntaxLicenseDirectory = Join-Path $releaseDirectory 'assets/syntaxes'
    New-Item -ItemType Directory -Path $syntaxLicenseDirectory -Force | Out-Null
    foreach ($syntaxDocumentation in @('DART-LICENSE.txt', 'README.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "assets/syntaxes/$syntaxDocumentation") -Destination $syntaxLicenseDirectory -Force
    }
    $windowLicenseDirectory = Join-Path $releaseDirectory 'licenses/winit'
    New-Item -ItemType Directory -Path $windowLicenseDirectory -Force | Out-Null
    foreach ($windowDocumentation in @('LICENSE', 'MORROW_PATCH.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "vendor/winit/$windowDocumentation") -Destination $windowLicenseDirectory -Force
    }
    if ($env:OS -eq 'Windows_NT') {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut((Join-Path $projectRoot 'Morrow.lnk'))
        $shortcut.TargetPath = $executable
        $shortcut.WorkingDirectory = $projectRoot
        $shortcut.Description = 'Morrow - Vim editor'
        $shortcut.Save()
    }
    Write-Output "Morrow is ready: $executable"
}
catch {
    $releaseFailure = $_
    foreach ($preservedBinary in $preservedBinaries) {
        if (-not (Test-Path -LiteralPath $preservedBinary.Original) -and (Test-Path -LiteralPath $preservedBinary.Backup)) {
            Move-Item -LiteralPath $preservedBinary.Backup -Destination $preservedBinary.Original -ErrorAction Stop
        }
    }
    throw $releaseFailure
}
finally {
    Pop-Location
}
