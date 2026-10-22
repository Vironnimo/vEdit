$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
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
    $packagedReadme = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'README.md')).Replace('(.agents/skills/vedit/SKILL.md)', '(skills/vedit/SKILL.md)')
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
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/agent-interface.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/cli-audit-0.4.19.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/development.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/maintenance-0.4.16.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'AGENTS.md') -Destination $releaseDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/user-review-0.4.9.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/code-review-0.4.10.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/usability-review-0.4.11.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/polyglot-keyboard-review-0.4.12.md') -Destination $documentationDirectory -Force
    $packagedPolish = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'docs/interaction-polish-0.4.13.md')).Replace('../vendor/winit/VEDIT_PATCH.md', '../licenses/winit/VEDIT_PATCH.md')
    [System.IO.File]::WriteAllText((Join-Path $documentationDirectory 'interaction-polish-0.4.13.md'), $packagedPolish, [System.Text.UTF8Encoding]::new($false))
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/ux-fixes-0.4.14.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/idle-performance-0.4.19.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/performance-audit-0.4.20.md') -Destination $documentationDirectory -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/performance-audit-0.4.25.md') -Destination $documentationDirectory -Force
    $packagedChrome = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'docs/window-chrome-0.4.15.md')).Replace('../vendor/winit/VEDIT_PATCH.md', '../licenses/winit/VEDIT_PATCH.md')
    [System.IO.File]::WriteAllText((Join-Path $documentationDirectory 'window-chrome-0.4.15.md'), $packagedChrome, [System.Text.UTF8Encoding]::new($false))
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
    $packagedScripts = Join-Path $releaseDirectory 'scripts'
    New-Item -ItemType Directory -Path $packagedScripts -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination $packagedScripts -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/vedit.ico') -Destination $releaseDirectory -Force
    $packageVersion = (Get-Content -LiteralPath (Join-Path $projectRoot 'Cargo.toml') | Select-String '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
    $packageFiles = [ordered]@{}
    Get-ChildItem -LiteralPath $releaseDirectory -Recurse -File | Where-Object {
        $_.Name -ne 'package.json' -and $_.Name -notlike 'vedit-running-*.exe'
    } | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($releaseDirectory.Length + 1)
        $packageFiles[$relative] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }
    @{ product = 'vEdit'; version = $packageVersion; files = $packageFiles } |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $releaseDirectory 'package.json') -Encoding UTF8
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
