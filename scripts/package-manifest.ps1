# Shared by the release builder and isolated installer tests.
function Write-PackageManifest([string]$Directory, [string]$Version) {
    $packageRoot = [IO.Path]::GetFullPath($Directory).TrimEnd('\', '/')
    $manifestPath = Join-Path $packageRoot 'package.json'
    $packageFiles = [ordered]@{}
    Get-ChildItem -LiteralPath $packageRoot -Recurse -File | Where-Object {
        # npm package metadata is part of the payload; only this root manifest
        # is excluded because it cannot contain its own hash.
        $_.FullName -ne $manifestPath -and $_.Name -notlike 'vedit-running-*.exe'
    } | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($packageRoot.Length + 1)
        $packageFiles[$relative] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }
    @{ product = 'vEdit'; version = $Version; files = $packageFiles } |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}
