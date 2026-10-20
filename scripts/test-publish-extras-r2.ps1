$ErrorActionPreference = 'Stop'
$work = Join-Path ([IO.Path]::GetTempPath()) ('optinr-publish-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
$badFile = Join-Path $work 'MFGUnlocker.asi'
try {
    [IO.File]::WriteAllText($badFile, 'corrupted resource')
    $rejected = $false
    try {
        & (Join-Path $PSScriptRoot 'publish-extras-r2.ps1') -SourceDir $work -CheckOnly
    } catch {
        if ($_.Exception.Message -notlike 'Size or SHA-256 mismatch:*MFGUnlocker.asi') { throw }
        $rejected = $true
    }
    if (-not $rejected) { throw 'Publisher accepted a corrupted resource' }
    Write-Host 'PASS: corrupted resources are rejected before any upload'
} finally {
    Remove-Item -LiteralPath $badFile -Force
    Remove-Item -LiteralPath $work
}
