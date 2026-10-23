param([Parameter(Mandatory = $true)][string]$ReleaseDirectory)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
$projectTemp = Enter-ProjectTemp
try {
$projectRoot = Split-Path -Parent $PSScriptRoot
$packageRoot = Join-Path $projectRoot 'tools/basedpyright'
$packageManifest = Get-Content -LiteralPath (Join-Path $packageRoot 'package.json') -Raw | ConvertFrom-Json
$installedManifest = Join-Path $packageRoot 'node_modules/basedpyright/package.json'
$installedVersion = if (Test-Path -LiteralPath $installedManifest) {
    (Get-Content -LiteralPath $installedManifest -Raw | ConvertFrom-Json).version
} else { '' }
if ($installedVersion -ne $packageManifest.dependencies.basedpyright) {
    & npm.cmd ci --prefix $packageRoot --ignore-scripts --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw 'Could not install the pinned Python language server.' }
    $installedVersion = $packageManifest.dependencies.basedpyright
}

$toolsDirectory = Join-Path $ReleaseDirectory 'tools'
$destination = Join-Path $toolsDirectory 'basedpyright'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $packageRoot 'node_modules') -Destination $destination -Recurse -Force
foreach ($name in @('package.json', 'package-lock.json', 'README.md')) {
    Copy-Item -LiteralPath (Join-Path $packageRoot $name) -Destination $destination -Force
}

# Keep this Python support usable when the portable directory is moved. Use the
# installed Node runtime, and ship its matching upstream license alongside it.
$nodePath = (Get-Command node.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$nodeVersion = (& $nodePath --version).Trim()
if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v\d+\.\d+\.\d+$') {
    throw 'Could not identify the Node runtime for Python language support.'
}
$licensePath = Join-Path $projectTemp.Root "node-$nodeVersion-LICENSE.txt"
if (-not (Test-Path -LiteralPath $licensePath)) {
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/nodejs/node/$nodeVersion/LICENSE" -OutFile $licensePath
}
$packagedNodePath = Join-Path $toolsDirectory 'node.exe'
# A running language server locks its executable. Identical bytes already form
# the requested bundle, so leave that runtime in place without stopping it.
$nodeAlreadyPackaged = (Test-Path -LiteralPath $packagedNodePath -PathType Leaf) -and
    ((Get-FileHash -LiteralPath $nodePath).Hash -eq (Get-FileHash -LiteralPath $packagedNodePath).Hash)
if (-not $nodeAlreadyPackaged) {
    Copy-Item -LiteralPath $nodePath -Destination $packagedNodePath -Force
}
Copy-Item -LiteralPath $licensePath -Destination (Join-Path $toolsDirectory 'node-LICENSE.txt') -Force
Set-Content -LiteralPath (Join-Path $toolsDirectory 'node-version.txt') -Value $nodeVersion -Encoding utf8
Write-Output "Python language support packaged: basedpyright $installedVersion, Node $nodeVersion"
} finally {
    Exit-ProjectTemp $projectTemp
}
