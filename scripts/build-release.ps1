$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
. (Join-Path $PSScriptRoot 'package-manifest.ps1')
$projectTemp = Enter-ProjectTemp
try {
$projectRoot = Split-Path -Parent $PSScriptRoot
$releaseDirectory = Join-Path $projectRoot 'dist/vEdit'
$preservedBinaries = [System.Collections.Generic.List[object]]::new()

function Remove-ExitedEditorBackups([string]$Directory) {
    # A backup stays while any process with its PID runs; a reused PID only delays removal.
    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) { return }
    Get-ChildItem -LiteralPath $Directory -File -Filter 'vedit-running-*.exe' |
        Where-Object { $_.Name -match '^vedit-running-(\d+)\.exe$' } |
        ForEach-Object {
            $backupFile = $_
            $processId = [int]$Matches[1]
            if (Get-Process -Id $processId -ErrorAction SilentlyContinue) { return }
            try {
                Remove-Item -LiteralPath $backupFile.FullName -ErrorAction Stop
                Write-Output "Removed the backup of an exited editor: $($backupFile.Name)"
            }
            catch {
                Write-Warning "Could not remove $($backupFile.FullName): $($_.Exception.Message)"
            }
        }
}

function Preserve-RunningEditor([string]$BinaryPath) {
    $absoluteBinary = [System.IO.Path]::GetFullPath($BinaryPath)
    $workspacePrefix = [System.IO.Path]::GetFullPath($projectRoot).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $absoluteBinary.StartsWith($workspacePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'The release binary must remain inside the project workspace.'
    }
    Remove-ExitedEditorBackups (Split-Path -Parent $absoluteBinary)
    if (-not (Test-Path -LiteralPath $absoluteBinary -PathType Leaf)) { return }
    try {
        $probe = [System.IO.File]::Open($absoluteBinary, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $probe.Dispose()
        return
    }
    catch [System.IO.IOException] {
        $runningEditor = Get-Process -Name 'vedit' -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and [System.IO.Path]::GetFullPath($_.Path) -eq $absoluteBinary } |
            Select-Object -First 1
        if (-not $runningEditor) { throw }
        $backup = Join-Path (Split-Path -Parent $absoluteBinary) "vedit-running-$($runningEditor.Id).exe"
        if (Test-Path -LiteralPath $backup) { throw "The running editor backup already exists: $backup" }
        Move-Item -LiteralPath $absoluteBinary -Destination $backup -ErrorAction Stop
        $preservedBinaries.Add([pscustomobject]@{ Original = $absoluteBinary; Backup = $backup })
        Write-Output "Preserved the running editor (PID $($runningEditor.Id)); its window and documents remain open."
    }
}

Push-Location $projectRoot
try {
    Preserve-RunningEditor (Join-Path $projectRoot 'target/release/vedit.exe')
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $executable = Join-Path $releaseDirectory 'vedit.exe'
    Preserve-RunningEditor $executable
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/vedit.exe') -Destination $executable -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/vedit-cli.exe') -Destination $releaseDirectory -Force
    & (Join-Path $PSScriptRoot 'package-language-tools.ps1') -ReleaseDirectory $releaseDirectory
    # The package README is the manual moved from docs/ to the package root:
    # sibling docs gain docs/, parent paths lose ../ and the skill sits in skills/.
    $packagedReadme = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'docs/handbuch.md'))
    $packagedReadme = [regex]::Replace($packagedReadme, '\]\((?!https?:|#|\.\./)', '](docs/')
    $packagedReadme = $packagedReadme.Replace('](../.agents/skills/vedit/', '](skills/vedit/').Replace('](../', '](')
    [System.IO.File]::WriteAllText((Join-Path $releaseDirectory 'README.md'), $packagedReadme, [System.Text.UTF8Encoding]::new($false))
    $previewDirectory = Join-Path $releaseDirectory 'tests/snapshots'
    New-Item -ItemType Directory -Path $previewDirectory -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'tests/snapshots') -File |
        Where-Object { $_.Name -match '^vedit_[a-z0-9_]+\.png$' } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $previewDirectory -Force }
    $skillDirectory = Join-Path $releaseDirectory 'skills/vedit'
    New-Item -ItemType Directory -Path $skillDirectory -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/vedit/SKILL.md') -Destination $skillDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/vedit/agents') -Destination $skillDirectory -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot '.agents/skills/vedit/references') -Destination $skillDirectory -Recurse -Force
    $documentationDirectory = Join-Path $releaseDirectory 'docs'
    New-Item -ItemType Directory -Path $documentationDirectory -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'docs') -File -Filter '*.md' | ForEach-Object {
        $packagedDocument = [System.IO.File]::ReadAllText($_.FullName).
            Replace('../vendor/winit/VEDIT_PATCH.md', '../licenses/winit/VEDIT_PATCH.md').
            Replace('../vendor/egui-winit/VEDIT_PATCH.md', '../licenses/egui-winit/VEDIT_PATCH.md').
            Replace('../vendor/eframe/VEDIT_PATCH.md', '../licenses/eframe/VEDIT_PATCH.md').
            Replace('../.agents/skills/vedit/SKILL.md', '../skills/vedit/SKILL.md').
            Replace('- [Project and domain index](../.vorch/PROJECT.md): source owners and task-specific working context.', '- Project and domain maps are available in the source checkout.')
        [System.IO.File]::WriteAllText((Join-Path $documentationDirectory $_.Name), $packagedDocument, [System.Text.UTF8Encoding]::new($false))
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'AGENTS.md') -Destination $releaseDirectory -Force
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
    $tokenizerLicenseDirectory = Join-Path $releaseDirectory 'licenses/tokenizer'
    New-Item -ItemType Directory -Path $tokenizerLicenseDirectory -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'licenses/tokenizer') -File |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $tokenizerLicenseDirectory -Force }
    New-Item -ItemType Directory -Path $windowLicenseDirectory -Force | Out-Null
    foreach ($windowDocumentation in @('LICENSE', 'VEDIT_PATCH.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "vendor/winit/$windowDocumentation") -Destination $windowLicenseDirectory -Force
    }
    $inputLicenseDirectory = Join-Path $releaseDirectory 'licenses/egui-winit'
    New-Item -ItemType Directory -Path $inputLicenseDirectory -Force | Out-Null
    foreach ($inputDocumentation in @('LICENSE-APACHE', 'LICENSE-MIT', 'VEDIT_PATCH.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "vendor/egui-winit/$inputDocumentation") -Destination $inputLicenseDirectory -Force
    }
    $frameLicenseDirectory = Join-Path $releaseDirectory 'licenses/eframe'
    New-Item -ItemType Directory -Path $frameLicenseDirectory -Force | Out-Null
    foreach ($frameDocumentation in @('LICENSE-APACHE', 'LICENSE-MIT', 'VEDIT_PATCH.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "vendor/eframe/$frameDocumentation") -Destination $frameLicenseDirectory -Force
    }
    $packagedScripts = Join-Path $releaseDirectory 'scripts'
    New-Item -ItemType Directory -Path $packagedScripts -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination $packagedScripts -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/vedit.ico') -Destination $releaseDirectory -Force
    $packageVersion = (Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') | Select-String '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
    Write-PackageManifest $releaseDirectory $packageVersion
    if ($env:OS -eq 'Windows_NT') {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut((Join-Path $projectRoot 'vEdit.lnk'))
        $shortcut.TargetPath = $executable
        $shortcut.WorkingDirectory = $projectRoot
        $shortcut.Description = 'vEdit - Vim editor'
        $shortcut.IconLocation = $executable + ',0'
        $shortcut.Save()
        # Retire only this checkout's former launcher, after the new one exists.
        $legacyShortcutPath = Join-Path $projectRoot 'Morrow.lnk'
        if (Test-Path -LiteralPath $legacyShortcutPath -PathType Leaf) {
            $legacyShortcut = $shell.CreateShortcut($legacyShortcutPath)
            $legacyExecutable = Join-Path $projectRoot 'dist/Morrow/morrow.exe'
            if ($legacyShortcut.TargetPath -eq $legacyExecutable) {
                Remove-Item -LiteralPath $legacyShortcutPath
            }
        }
    }
    Write-Output "vEdit is ready: $executable"
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
} finally {
    Exit-ProjectTemp $projectTemp
}
