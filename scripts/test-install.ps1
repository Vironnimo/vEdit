$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
. (Join-Path $PSScriptRoot 'package-manifest.ps1')
$projectTemp = Enter-ProjectTemp
try {
$testId = [Guid]::NewGuid().ToString('N')
$fixture = Join-Path $projectTemp.Root "vedit-installer-tests/$testId"
$package = Join-Path $fixture 'package'
$installed = Join-Path $fixture 'installed app'
$installer = Join-Path $PSScriptRoot 'install.ps1'
$registryPath = "Software\vEditInstallerTests\$testId"
$checks = 0
function Assert($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}
function Manifest {
    Write-PackageManifest $package '0.0.1-test'
}
function Install { & $installer -PackagePath $package -InstallDirectory $installed -TestRoot $fixture }
function RegistryValue([string]$Key, [string]$Name = '') {
    $handle = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("$registryPath\$Key")
    if ($handle) { try { $handle.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } finally { $handle.Dispose() } }
}
$helper = $null
$locked = $null
$originalProcessPath = $env:PATH
try {
    New-Item -ItemType Directory -Path (Join-Path $package 'scripts'), (Join-Path $package 'tools/basedpyright/node_modules/basedpyright') -Force | Out-Null
    Copy-Item -LiteralPath $installer -Destination (Join-Path $package 'scripts/install.ps1')
    Copy-Item -LiteralPath (Join-Path $env:WINDIR 'System32/ping.exe') -Destination (Join-Path $package 'vedit.exe')
    Copy-Item -LiteralPath (Join-Path $env:WINDIR 'System32/whoami.exe') -Destination (Join-Path $package 'vedit-cli.exe')
    foreach ($file in @('00-rollback.txt', 'vedit.ico', 'tools/node.exe', 'tools/basedpyright/node_modules/basedpyright/langserver.index.js')) {
        Set-Content -LiteralPath (Join-Path $package $file) -Value 'fixture bytes'
    }
    $nestedManifest = 'tools/basedpyright/node_modules/basedpyright/package.json'
    Set-Content -LiteralPath (Join-Path $package $nestedManifest) -Value '{"name":"basedpyright","version":"fixture"}'
    $environment = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("$registryPath\Environment")
    $environment.SetValue('Path', 'C:\keep;%USERPROFILE%\bin', [Microsoft.Win32.RegistryValueKind]::ExpandString)
    $environment.Dispose()
    $association = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("$registryPath\Software\Classes\.txt")
    $association.SetValue('', 'Keep.ExistingDefault')
    $association.Dispose()
    Manifest
    $manifestFiles = (Get-Content -LiteralPath (Join-Path $package 'package.json') -Raw | ConvertFrom-Json).files.PSObject.Properties.Name
    Assert ($nestedManifest.Replace('/', '\') -in $manifestFiles) 'Release manifest omitted nested npm package metadata.'
    Assert ('package.json' -notin $manifestFiles) 'Release manifest must not hash itself.'
    Install
    Assert ((Get-FileHash -LiteralPath (Join-Path $installed $nestedManifest)).Hash -eq (Get-FileHash -LiteralPath (Join-Path $package $nestedManifest)).Hash) 'Installed npm metadata differs.'
    Assert ((Get-FileHash -LiteralPath (Join-Path $installed 'vedit.exe')).Hash -eq (Get-FileHash -LiteralPath (Join-Path $package 'vedit.exe')).Hash) 'Installed executable differs.'
    Assert ((RegistryValue 'Software\Classes\.txt') -eq 'Keep.ExistingDefault') 'Existing default changed.'
    Assert ((RegistryValue 'Software\RegisteredApplications' 'vEdit') -eq 'Software\vEdit\Capabilities') 'Default-app registration missing.'
    Assert ((RegistryValue 'Software\Classes\vEdit.Text\shell\open\command') -eq ('"' + (Join-Path $installed 'vedit.exe') + '" -- "%1"')) 'Open command does not quote paths or terminate options.'
    Assert ((RegistryValue 'Software\vEdit\Capabilities\FileAssociations' '.txt') -eq 'vEdit.Text') 'Text capability missing.'
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $fixture 'vEdit.lnk'))
    Assert ($shortcut.TargetPath -eq (Join-Path $installed 'vedit.exe')) 'Shortcut points outside installation.'
    Assert ($shortcut.IconLocation -eq ((Join-Path $installed 'vedit.exe') + ',0')) 'Shortcut icon must use an unquoted path to the executable resource.'
    Install
    $path = RegistryValue 'Environment' 'Path'
    Assert ($path -eq "C:\keep;%USERPROFILE%\bin;$installed") 'Repeated install duplicated PATH or lost expandable entries.'
    $nestedBefore = [IO.File]::ReadAllBytes((Join-Path $package $nestedManifest))
    Set-Content -LiteralPath (Join-Path $package $nestedManifest) -Value '{"name":"tampered"}'
    $blocked = $false
    try { Install } catch { $blocked = $true }
    Assert $blocked 'Tampered npm package metadata was accepted.'
    [IO.File]::WriteAllBytes((Join-Path $package $nestedManifest), $nestedBefore)
    # A copied Windows ping is a disposable running executable; no editor session
    # is used. It exits on its own after the update check.
    $helper = Start-Process -FilePath (Join-Path $installed 'vedit.exe') -ArgumentList @('-n', '12', '127.0.0.1') -WindowStyle Hidden -PassThru
    Copy-Item -LiteralPath (Join-Path $env:WINDIR 'System32/whoami.exe') -Destination (Join-Path $package 'vedit.exe') -Force
    Manifest
    Install
    Assert (-not $helper.HasExited) 'Update terminated the running executable.'
    Assert ((Get-FileHash -LiteralPath (Join-Path $installed 'vedit.exe')).Hash -eq (Get-FileHash -LiteralPath (Join-Path $package 'vedit.exe')).Hash) 'Updated executable differs.'
    $blocked = $false
    try { & $installer -Uninstall -InstallDirectory $installed -TestRoot $fixture } catch { $blocked = $true }
    Assert $blocked 'Uninstall allowed a running installed process.'
    $helper.WaitForExit(20000) | Out-Null
    Assert $helper.HasExited 'Fixture executable did not exit.'
    Install
    Assert (@(Get-ChildItem -LiteralPath $installed -Recurse -Filter '*.vedit-backup-*').Count -eq 0) 'Exited executable backup was not cleaned.'
    # A later locked file makes the transaction roll back earlier replacements.
    $before = (Get-FileHash -LiteralPath (Join-Path $installed 'vedit.exe')).Hash
    $rollbackBefore = (Get-FileHash -LiteralPath (Join-Path $installed '00-rollback.txt')).Hash
    Set-Content -LiteralPath (Join-Path $package '00-rollback.txt') -Value 'replacement before the locked runtime'
    Copy-Item -LiteralPath (Join-Path $env:WINDIR 'System32/ping.exe') -Destination (Join-Path $package 'vedit.exe') -Force
    Set-Content -LiteralPath (Join-Path $package 'tools/node.exe') -Value 'changed runtime'
    Manifest
    $locked = [IO.File]::Open((Join-Path $installed 'tools/node.exe'), 'Open', 'Read', 'None')
    $blocked = $false
    try { Install } catch { $blocked = $true }
    $locked.Dispose(); $locked = $null
    Assert $blocked 'Locked runtime did not reject update.'
    Assert ((Get-FileHash -LiteralPath (Join-Path $installed '00-rollback.txt')).Hash -eq $rollbackBefore) 'Failed update did not roll back an already replaced file.'
    Assert ((Get-FileHash -LiteralPath (Join-Path $installed 'vedit.exe')).Hash -eq $before) 'Failed update did not restore previous executable.'
    Set-Content -LiteralPath (Join-Path $package 'vedit-cli.exe') -Value 'tampered'
    $blocked = $false
    try { Install } catch { $blocked = $true }
    Assert $blocked 'Tampered package was accepted.'
    $sentinel = Join-Path $installed 'user-note.txt'
    Set-Content -LiteralPath $sentinel -Value 'keep me'
    & $installer -Uninstall -InstallDirectory $installed -TestRoot $fixture
    Assert (Test-Path -LiteralPath $sentinel) 'Uninstall deleted an unknown file.'
    Assert (-not (Test-Path -LiteralPath (Join-Path $installed 'vedit.exe'))) 'Uninstall kept the owned executable.'
    Assert ((RegistryValue 'Environment' 'Path') -eq 'C:\keep;%USERPROFILE%\bin') 'Uninstall damaged existing PATH entries.'
    Assert ((RegistryValue 'Software\Classes\.txt') -eq 'Keep.ExistingDefault') 'Uninstall deleted another application association.'
    Assert (-not (RegistryValue 'Software\RegisteredApplications' 'vEdit')) 'Uninstall kept registration.'
    Write-Output "$checks installer checks passed in an isolated filesystem and registry fixture."
} finally {
    if ($locked) { $locked.Dispose() }
    if ($helper -and -not $helper.HasExited) { $helper.WaitForExit(20000) | Out-Null }
    $env:PATH = $originalProcessPath
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($registryPath, $false)
    $resolved = [IO.Path]::GetFullPath($fixture)
    $testPrefix = [IO.Path]::GetFullPath((Join-Path $projectTemp.Root 'vedit-installer-tests')) + '\'
    if ($resolved.StartsWith($testPrefix, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -eq $testId) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
} finally {
    Exit-ProjectTemp $projectTemp
}
