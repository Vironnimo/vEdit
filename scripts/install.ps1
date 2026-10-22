[CmdletBinding()]
param(
    [string]$PackagePath,
    [string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'Programs/vEdit'),
    [switch]$Uninstall,
    [switch]$OpenDefaultApps,
    [Parameter(DontShow)][string]$TestRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$installIdentity = [IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\', '/').ToLowerInvariant()
$hasher = [Security.Cryptography.SHA256]::Create()
try { $lockId = [BitConverter]::ToString($hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($installIdentity))).Replace('-', '') }
finally { $hasher.Dispose() }
$installMutex = [Threading.Mutex]::new($false, "Local\vEdit.Install.$lockId")
$ownsMutex = $false
try {
    try { $ownsMutex = $installMutex.WaitOne(0) }
    catch [Threading.AbandonedMutexException] { $ownsMutex = $true }
    if (-not $ownsMutex) { throw 'Another vEdit installation or update is already running for this directory.' }

# This is also shipped in the package. From a checkout, the same command builds
# first; from an extracted package it installs that package without Rust or npm.
$sourceRoot = Split-Path -Parent $PSScriptRoot
if (-not $Uninstall -and -not $PackagePath) {
    if (Test-Path -LiteralPath (Join-Path $sourceRoot 'Cargo.toml')) {
        & (Join-Path $PSScriptRoot 'build-release.ps1')
        $PackagePath = Join-Path $sourceRoot 'dist/vEdit'
    } else {
        $PackagePath = $sourceRoot
    }
}
$installRoot = [IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\', '/')
$markerPath = Join-Path $installRoot 'installation.json'
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'vEdit.lnk'
$classes = 'Software\Classes'
$appKey = 'Software\vEdit'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\vEdit'
$appPathsKey = 'Software\Microsoft\Windows\CurrentVersion\App Paths\vedit.exe'
$extensions = @('.txt', '.text', '.log', '.md', '.markdown', '.json', '.jsonc', '.toml', '.yaml', '.yml', '.xml', '.ini', '.cfg', '.conf', '.csv', '.tsv', '.rs', '.py', '.pyi', '.pyw', '.dart', '.js', '.jsx', '.ts', '.tsx', '.html', '.htm', '.css', '.scss', '.sql', '.sh', '.ps1', '.bat', '.cmd', '.c', '.h', '.cpp', '.hpp', '.java', '.go', '.rb', '.lua', '.tex')
$userRegistry = [Microsoft.Win32.Registry]::CurrentUser
if ($TestRoot) {
    # Integration tests redirect every Windows registration and shortcut away
    # from the real installation. The disposable fixture's leaf is a GUID.
    $testId = [Guid]::Parse((Split-Path -Leaf $TestRoot)).ToString('N')
    $userRegistry = $userRegistry.CreateSubKey("Software\vEditInstallerTests\$testId")
    $shortcutPath = Join-Path $TestRoot 'vEdit.lnk'
}

function Set-RegistryValue([string]$Key, [string]$Name, $Value, [Microsoft.Win32.RegistryValueKind]$Kind = 'String') {
    $handle = $userRegistry.CreateSubKey($Key)
    try { $handle.SetValue($Name, $Value, $Kind) } finally { $handle.Dispose() }
}

function Get-InstalledPath([string]$Relative) {
    $path = [IO.Path]::GetFullPath((Join-Path $installRoot $Relative))
    if (-not $path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Package path escapes the installation: $Relative"
    }
    $path
}

function Set-UserPath([bool]$Add) {
    $environmentKey = $userRegistry.CreateSubKey('Environment')
    try {
        $old = [string]$environmentKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $parts = @($old.Split(';') | Where-Object {
            $_ -and [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\', '/') -ine $installRoot
        })
        if ($Add) { $parts += $installRoot }
        # Preserve expandable entries such as %USERPROFILE% instead of writing a
        # snapshot of the combined machine/process PATH back to the user value.
        $environmentKey.SetValue('Path', ($parts -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
    } finally { $environmentKey.Dispose() }
    $processParts = @($env:PATH.Split(';') | Where-Object { $_.TrimEnd('\', '/') -ine $installRoot })
    if ($Add) { $env:PATH = (@($installRoot) + $processParts) -join ';' }
    else { $env:PATH = $processParts -join ';' }
}

function Notify-Shell {
    if ($TestRoot) { return }
    if (-not ('VEdit.InstallShell' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace VEdit {
    public static class InstallShell {
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern IntPtr SendMessageTimeout(IntPtr window, uint message, UIntPtr wparam, string lparam, uint flags, uint timeout, out UIntPtr result);
        [DllImport("shell32.dll")]
        public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
    }
}
'@
    }
    $result = [UIntPtr]::Zero
    [void][VEdit.InstallShell]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 3000, [ref]$result)
    [VEdit.InstallShell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

$oldManifest = $null
if (Test-Path -LiteralPath $markerPath) {
    $oldManifest = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    if ($oldManifest.product -ne 'vEdit' -or $oldManifest.directory -ine $installRoot) {
        throw 'The installation marker does not belong to this vEdit directory.'
    }
}

if ($Uninstall) {
    if (-not $oldManifest) { throw "No managed vEdit installation exists at $installRoot." }
    $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
    })
    if ($running.Count) { throw 'Close vEdit and its language tools normally before uninstalling. Nothing was removed.' }
    $registered = $userRegistry.OpenSubKey($appKey)
    $ownsRegistration = $false
    if ($registered) {
        try { $ownsRegistration = $registered.GetValue('InstallDirectory') -ieq $installRoot } finally { $registered.Dispose() }
    }
    if ($ownsRegistration) {
        foreach ($extension in $extensions) {
            $key = $userRegistry.OpenSubKey("$classes\$extension\OpenWithProgids", $true)
            if ($key) { try { $key.DeleteValue('vEdit.Text', $false) } finally { $key.Dispose() } }
        }
        foreach ($key in @("$classes\vEdit.Text", "$classes\Applications\vedit.exe", $appPathsKey, $uninstallKey, $appKey)) {
            $userRegistry.DeleteSubKeyTree($key, $false)
        }
        $registeredApps = $userRegistry.OpenSubKey('Software\RegisteredApplications', $true)
        if ($registeredApps) { try { $registeredApps.DeleteValue('vEdit', $false) } finally { $registeredApps.Dispose() } }
        if (Test-Path -LiteralPath $shortcutPath) {
            $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
            if ($shortcut.TargetPath -ieq (Join-Path $installRoot 'vedit.exe')) { Remove-Item -LiteralPath $shortcutPath }
        }
    }
    Set-UserPath $false
    # Remove only manifest-owned files. User profiles and unknown files are never
    # recursively removed, even if somebody selected an unusual install folder.
    foreach ($relative in @($oldManifest.files)) {
        $path = Get-InstalledPath $relative
        if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path }
    }
    Remove-Item -LiteralPath $markerPath
    Get-ChildItem -LiteralPath $installRoot -Recurse -File -Filter '*.vedit-backup-*' | ForEach-Object {
        Remove-Item -LiteralPath (Get-InstalledPath $_.FullName.Substring($installRoot.Length + 1))
    }
    Notify-Shell
    Write-Output 'vEdit uninstalled. Settings, sessions and recovery files were kept.'
    if ($TestRoot) { $userRegistry.Dispose() }
    return
}

$packageRoot = (Resolve-Path -LiteralPath $PackagePath).Path.TrimEnd('\', '/')
if ($packageRoot -ieq $installRoot -or $installRoot.StartsWith($packageRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or $packageRoot.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Extract the new package outside the installation before installing an update.'
}
foreach ($required in @('vedit.exe', 'vedit-cli.exe', 'vedit.ico', 'package.json', 'tools/node.exe', 'tools/basedpyright/node_modules/basedpyright/langserver.index.js', 'scripts/install.ps1')) {
    if (-not (Test-Path -LiteralPath (Join-Path $packageRoot $required) -PathType Leaf)) { throw "Incomplete vEdit package: missing $required" }
}
if ((Test-Path -LiteralPath $installRoot) -and -not $oldManifest -and @(Get-ChildItem -LiteralPath $installRoot -Force).Count) {
    throw 'The destination is not empty and has no vEdit installation marker. Choose an empty directory.'
}
$package = Get-Content -LiteralPath (Join-Path $packageRoot 'package.json') -Raw | ConvertFrom-Json
if ($package.product -ne 'vEdit') { throw 'This is not a vEdit package.' }
$files = @($package.files.PSObject.Properties)
foreach ($required in @('vedit.exe', 'vedit-cli.exe', 'vedit.ico', 'tools\node.exe', 'tools\basedpyright\node_modules\basedpyright\langserver.index.js', 'scripts\install.ps1')) {
    if ($required -notin $files.Name) { throw "Package manifest does not verify $required." }
}
foreach ($file in $files) {
    $source = [IO.Path]::GetFullPath((Join-Path $packageRoot $file.Name))
    if (-not $source.StartsWith($packageRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid package file path.' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $file.Value) { throw "Package verification failed: $($file.Name)" }
    [void](Get-InstalledPath $file.Name)
}
New-Item -ItemType Directory -Path $installRoot -Force | Out-Null
if (-not $oldManifest) {
    @{ product = 'vEdit'; version = $package.version; directory = $installRoot; files = @() } |
        ConvertTo-Json | Set-Content -LiteralPath $markerPath -Encoding UTF8
}
$transaction = [Guid]::NewGuid().ToString('N')
$journal = [Collections.Generic.List[object]]::new()
try {
    # Each replacement is staged beside its destination, then renamed. A running
    # GUI keeps its renamed executable, just as in build-release.ps1.
    foreach ($relative in (@($files.Name) + @('package.json'))) {
        $destination = Get-InstalledPath $relative
        $source = Join-Path $packageRoot $relative
        if ((Test-Path -LiteralPath $destination) -and (Get-FileHash -LiteralPath $source).Hash -eq (Get-FileHash -LiteralPath $destination).Hash) { continue }
        New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
        $staged = "$destination.vedit-stage-$transaction"
        $backup = "$destination.vedit-backup-$transaction"
        Copy-Item -LiteralPath $source -Destination $staged
        $entry = [pscustomobject]@{ Destination = $destination; Backup = $backup; Staged = $staged; Replaced = $false; HadOriginal = $false }
        $journal.Add($entry)
        if (Test-Path -LiteralPath $destination) {
            Move-Item -LiteralPath $destination -Destination $backup
            $entry.HadOriginal = $true
        }
        Move-Item -LiteralPath $staged -Destination $destination
        $entry.Replaced = $true
    }
} catch {
    for ($index = $journal.Count - 1; $index -ge 0; $index--) {
        $entry = $journal[$index]
        if ($entry.Replaced) { Remove-Item -LiteralPath $entry.Destination }
        if ($entry.HadOriginal) { Move-Item -LiteralPath $entry.Backup -Destination $entry.Destination }
        if (Test-Path -LiteralPath $entry.Staged) { Remove-Item -LiteralPath $entry.Staged }
    }
    throw
}
$executable = Join-Path $installRoot 'vedit.exe'
$ownedFiles = @($files.Name) + @('package.json')
if ($oldManifest) { $ownedFiles += @($oldManifest.files) }
$manifest = @{ product = 'vEdit'; version = $package.version; directory = $installRoot; files = @($ownedFiles | Sort-Object -Unique) }
$manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $markerPath -Encoding UTF8
$icon = '"' + $executable + '",0'
$openCommand = '"' + $executable + '" -- "%1"'
Set-RegistryValue $appKey 'InstallDirectory' $installRoot
Set-RegistryValue "$appKey\Capabilities" 'ApplicationName' 'vEdit'
Set-RegistryValue "$appKey\Capabilities" 'ApplicationDescription' 'Text and code editor with projects, tabs and separate windows.'
Set-RegistryValue "$appKey\Capabilities" 'ApplicationIcon' $icon
Set-RegistryValue 'Software\RegisteredApplications' 'vEdit' "$appKey\Capabilities"
Set-RegistryValue "$classes\vEdit.Text" '' 'vEdit text document'
Set-RegistryValue "$classes\vEdit.Text\DefaultIcon" '' $icon
Set-RegistryValue "$classes\vEdit.Text\shell\open\command" '' $openCommand
Set-RegistryValue "$classes\Applications\vedit.exe" 'FriendlyAppName' 'vEdit'
Set-RegistryValue "$classes\Applications\vedit.exe\shell\open\command" '' $openCommand
foreach ($extension in $extensions) {
    Set-RegistryValue "$appKey\Capabilities\FileAssociations" $extension 'vEdit.Text'
    Set-RegistryValue "$classes\$extension\OpenWithProgids" 'vEdit.Text' ([byte[]]@()) 'None'
    Set-RegistryValue "$classes\Applications\vedit.exe\SupportedTypes" $extension ''
}
Set-RegistryValue $appPathsKey '' $executable
Set-RegistryValue $appPathsKey 'Path' $installRoot
$uninstallCommand = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $installRoot 'scripts/install.ps1') + '" -Uninstall -InstallDirectory "' + $installRoot + '"'
Set-RegistryValue $uninstallKey 'DisplayName' 'vEdit'
Set-RegistryValue $uninstallKey 'DisplayVersion' $package.version
Set-RegistryValue $uninstallKey 'DisplayIcon' $icon
Set-RegistryValue $uninstallKey 'InstallLocation' $installRoot
Set-RegistryValue $uninstallKey 'UninstallString' $uninstallCommand
Set-RegistryValue $uninstallKey 'NoModify' 1 'DWord'
Set-RegistryValue $uninstallKey 'NoRepair' 1 'DWord'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
$shortcut.TargetPath = $executable
$shortcut.WorkingDirectory = $installRoot
$shortcut.Description = 'vEdit text and code editor'
# IShellLink stores a filename, not a command line; quotes become part of it.
$shortcut.IconLocation = $executable + ',0'
$shortcut.Save()
Set-UserPath $true
foreach ($entry in $journal) {
    if ($entry.HadOriginal) {
        try { Remove-Item -LiteralPath $entry.Backup -ErrorAction Stop } catch { Write-Output "Kept a running file until the next update: $($entry.Backup)" }
    }
}
# Old backup filenames are installer-generated and stay within this managed root.
Get-ChildItem -LiteralPath $installRoot -Recurse -File -Filter '*.vedit-backup-*' | ForEach-Object {
    $backup = Get-InstalledPath $_.FullName.Substring($installRoot.Length + 1)
    try { Remove-Item -LiteralPath $backup -ErrorAction Stop } catch { }
}
Notify-Shell
Write-Output "Installed vEdit $($package.version) at $installRoot."
Write-Output 'Start menu, user PATH, Open with and Default apps registration are ready.'
Write-Output 'New terminals and newly started agent apps inherit PATH. Close and reopen vEdit normally to use an update.'
Write-Output 'Choose defaults in Windows Settings > Apps > Default apps > vEdit.'
if ($OpenDefaultApps -and -not $TestRoot) { Start-Process 'ms-settings:defaultapps?registeredAppUser=vEdit' }
if ($TestRoot) { $userRegistry.Dispose() }
} finally {
    if ($ownsMutex) { $installMutex.ReleaseMutex() }
    $installMutex.Dispose()
}
