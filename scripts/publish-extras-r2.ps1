param(
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [string]$Bucket = 'optiscaler-nr-assets',
    [string[]]$WranglerCommand = @('wrangler'),
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -LiteralPath (Join-Path $repo 'release/extras-v1.json') -Raw | ConvertFrom-Json
$source = (Resolve-Path -LiteralPath $SourceDir).Path
$baseUrl = [Uri]$manifest.base_url
if ($baseUrl.Scheme -ne 'https' -or $baseUrl.Query -or $baseUrl.Fragment -or
    -not $baseUrl.AbsoluteUri.EndsWith('/') -or $Bucket -notmatch '^[a-z0-9-]+$') {
    throw 'Invalid R2 bucket or HTTPS base URL'
}

function Assert-ResourceFile([string]$Path, $Spec) {
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer -or $file.Length -ne $Spec.size -or
        (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ine $Spec.sha256) {
        throw "Size or SHA-256 mismatch: $Path"
    }
}

$entries = @($manifest.files) + @($manifest.sources)
$keys = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$sourceFiles = @{}
foreach ($entry in $entries) {
    foreach ($relative in @($entry.path, $entry.key)) {
        if ($relative -notmatch '^[a-zA-Z0-9._/-]+$' -or $relative.StartsWith('/') -or
            $relative.EndsWith('/') -or ($relative -split '/') -contains '..' -or
            ($relative -split '/') -contains '.') { throw "Unsafe resource path: $relative" }
    }
    if (-not $keys.Add($entry.key) -or $entry.size -le 0 -or $entry.sha256 -notmatch '^[a-fA-F0-9]{64}$') {
        throw "Invalid or duplicate resource: $($entry.key)"
    }
    $path = Join-Path $source $entry.path
    if ($entry.path -eq 'EXTRAS-NOTICES.md' -and -not (Test-Path -LiteralPath $path)) {
        $path = Join-Path $repo 'release/EXTRAS-NOTICES.md'
    }
    Assert-ResourceFile $path $entry
    $sourceFiles[$entry.key] = $path
    Write-Host "Verified source: $($entry.path)"
}
if ($CheckOnly) { return }

$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$work = [IO.Path]::GetFullPath((Join-Path $tempBase ('optinr-r2-' + [Guid]::NewGuid().ToString('N'))))
New-Item -ItemType Directory -Path $work | Out-Null
$command = $WranglerCommand[0]
$commandArgs = @($WranglerCommand | Select-Object -Skip 1)
try {
    # shortcut: one publisher per version; serialize concurrent publishers before adding CI.
    foreach ($entry in $entries) {
        $key = $baseUrl.AbsolutePath.TrimStart('/') + $entry.key
        $objectPath = "$Bucket/$key"
        $remoteFile = Join-Path $work 'remote.bin'
        $result = & $command @commandArgs r2 object get $objectPath --remote --file $remoteFile 2>&1
        if ($LASTEXITCODE -eq 0) {
            Assert-ResourceFile $remoteFile $entry
            Write-Host "Already published and unchanged: $key"
        } elseif (($result -join "`n") -match 'The specified key does not exist\.') {
            $contentType = if ($entry.path.EndsWith('.md')) { 'text/plain; charset=utf-8' } else { 'application/octet-stream' }
            & $command @commandArgs r2 object put $objectPath --remote --file $sourceFiles[$entry.key] `
                --content-type $contentType --cache-control 'public,max-age=31536000,immutable'
            if ($LASTEXITCODE -ne 0) { throw "R2 upload failed: $key" }
        } else {
            throw "Could not inspect R2 object; refusing to overwrite: $key`n$($result -join "`n")"
        }
    }
    foreach ($entry in $entries) {
        $url = $baseUrl.AbsoluteUri + $entry.key
        $publicFile = Join-Path $work 'public.bin'
        $response = Invoke-WebRequest -Uri $url -OutFile $publicFile -PassThru -TimeoutSec 600
        Assert-ResourceFile $publicFile $entry
        $rangeFile = Join-Path $work 'range.bin'
        $range = Invoke-WebRequest -Uri $url -Headers @{ Range = 'bytes=0-1023' } -OutFile $rangeFile -PassThru -TimeoutSec 60
        if ($range.StatusCode -ne 206 -or
            ($range.Headers['Content-Range'] -join '') -ne "bytes 0-1023/$($entry.size)" -or
            (Get-Item -LiteralPath $rangeFile).Length -ne 1024) {
            throw "Invalid public Range response: $url"
        }
        Write-Host "Verified public download and Range: $url (cache: $($range.Headers['CF-Cache-Status'] -join ''))"
    }
} finally {
    if (-not $work.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe temporary directory' }
    Remove-Item -LiteralPath $work -Recurse -Force
}
