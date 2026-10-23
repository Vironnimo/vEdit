# Dot-source this helper; each caller restores its process environment in finally.
function Enter-ProjectTemp {
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../.tmp'))
    [IO.Directory]::CreateDirectory($root) | Out-Null
    $previous = @{}
    foreach ($name in @('TEMP', 'TMP', 'TMPDIR', 'GIT_CEILING_DIRECTORIES')) {
        $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    $context = [pscustomobject]@{ Root = $root; Previous = $previous }
    try {
        foreach ($name in $previous.Keys) {
            [Environment]::SetEnvironmentVariable($name, $root, 'Process')
        }
    } catch {
        Exit-ProjectTemp $context
        throw
    }
    $context
}

function Exit-ProjectTemp($Context) {
    foreach ($name in $Context.Previous.Keys) {
        [Environment]::SetEnvironmentVariable($name, $Context.Previous[$name], 'Process')
    }
}
