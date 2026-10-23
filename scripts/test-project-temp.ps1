$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'project-temp.ps1')
$names = @('TEMP', 'TMP', 'TMPDIR', 'GIT_CEILING_DIRECTORIES')
$original = @{}
foreach ($name in $names) { $original[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$expected = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../.tmp'))
$checks = 0
function Assert($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:checks++
}

# The helper must use its checkout even when invoked from another working directory.
Push-Location $PSScriptRoot
$projectTemp = $null
try {
    $projectTemp = Enter-ProjectTemp
    Assert ($projectTemp.Root -eq $expected) 'Temporary root depends on the working directory.'
    foreach ($name in $names) {
        Assert ([Environment]::GetEnvironmentVariable($name, 'Process') -eq $expected) "$name was not redirected."
    }
    Assert ([IO.Path]::GetTempPath().TrimEnd('\', '/') -eq $expected) '.NET still uses system Temp.'
    $childPath = & powershell.exe -NoProfile -Command '[IO.Path]::GetTempFileName()'
    Assert ($LASTEXITCODE -eq 0) 'Child process failed.'
    $childPath = [IO.Path]::GetFullPath($childPath)
    Assert ((Split-Path -Parent $childPath) -eq $expected) 'Child process escaped the project Temp directory.'
    Assert (Test-Path -LiteralPath $childPath -PathType Leaf) 'Child did not create its temporary file.'
    Remove-Item -LiteralPath $childPath

    # Nested scripts restore the outer scope even after a failing command.
    $nested = Enter-ProjectTemp
    try { throw 'Expected fixture failure' }
    catch { Assert ($_.Exception.Message -eq 'Expected fixture failure') 'Unexpected nested error.' }
    finally { Exit-ProjectTemp $nested }
    foreach ($name in $names) {
        Assert ([Environment]::GetEnvironmentVariable($name, 'Process') -eq $expected) "$name lost the outer scope."
    }
} finally {
    if ($projectTemp) { Exit-ProjectTemp $projectTemp }
    Pop-Location
}
foreach ($name in $names) {
    Assert ([Environment]::GetEnvironmentVariable($name, 'Process') -ceq $original[$name]) "$name was not restored."
}
Write-Output "$checks project Temp checks passed, including child processes and environment restoration."
