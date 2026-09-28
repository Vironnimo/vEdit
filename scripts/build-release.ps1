$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = Split-Path -Parent $PSScriptRoot
$releaseDirectory = Join-Path $projectRoot 'dist/Morrow'

Push-Location $projectRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    New-Item -ItemType Directory -Path $releaseDirectory -Force | Out-Null
    $executable = Join-Path $releaseDirectory 'morrow.exe'
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/morrow.exe') -Destination $executable -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot 'target/release/morrow-cli.exe') -Destination $releaseDirectory -Force
    $packagedReadme = [System.IO.File]::ReadAllText((Join-Path $projectRoot 'README.md')).Replace('(.agents/skills/morrow/SKILL.md)', '(skills/morrow/SKILL.md)')
    [System.IO.File]::WriteAllText((Join-Path $releaseDirectory 'README.md'), $packagedReadme, [System.Text.UTF8Encoding]::new($false))
    $previewDirectory = Join-Path $releaseDirectory 'tests/snapshots'
    New-Item -ItemType Directory -Path $previewDirectory -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'tests/snapshots') -File |
        Where-Object { $_.Name -match '^morrow_[a-z_]+\.png$' } |
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
    $fontLicenseDirectory = Join-Path $releaseDirectory 'assets/fonts'
    New-Item -ItemType Directory -Path $fontLicenseDirectory -Force | Out-Null
    foreach ($fontDocumentation in @('OFL.txt', 'README.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot "assets/fonts/$fontDocumentation") -Destination $fontLicenseDirectory -Force
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
finally {
    Pop-Location
}
