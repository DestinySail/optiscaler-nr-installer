param(
    [Parameter(Mandatory = $true)]
    [string]$SourceDir,
    [string]$Version = 'extras-maintenance-v1'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$source = (Resolve-Path -LiteralPath $SourceDir).Path
$required = @(
    (Join-Path $source 'nvngx_dlssnr.dll'),
    (Join-Path $source 'dlssnr_on_amd_weights.bin')
)
foreach ($item in $required) {
    if (-not (Test-Path -LiteralPath $item -PathType Leaf)) {
        throw "固定资源缺少 $item"
    }
}

$repo = Split-Path -Parent $PSScriptRoot
$repoRoot = $repo
$dist = Join-Path $repo 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$zip = Join-Path $dist "$Version.zip"
if (Test-Path -LiteralPath $zip) {
    Remove-Item -LiteralPath $zip -Force
}

$entries = $required | ForEach-Object { Get-Item -LiteralPath $_ }
$archive = [System.IO.Compression.ZipFile]::Open(
    $zip,
    [System.IO.Compression.ZipArchiveMode]::Create
)
try {
    foreach ($file in $entries) {
        $relative = $file.FullName.Substring($source.Length).TrimStart('\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $archive,
            $file.FullName,
            $relative,
            [System.IO.Compression.CompressionLevel]::Optimal
        ) | Out-Null
    }
    $notice = Join-Path $repoRoot 'release\EXTRAS-NOTICES.md'
    if (Test-Path -LiteralPath $notice) {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $archive,
            $notice,
            'EXTRAS-NOTICES.md',
            [System.IO.Compression.CompressionLevel]::Optimal
        ) | Out-Null
    }
}
finally {
    $archive.Dispose()
}

$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content -LiteralPath (Join-Path $dist "$Version.sha256") -Value "$hash  $Version.zip" -Encoding ascii
@("$Version.zip", "$Version.sha256") |
    ForEach-Object { Get-Item -LiteralPath (Join-Path $dist $_) } |
    ForEach-Object {
        $itemHash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$itemHash  $($_.Name)"
    } |
    Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii

Write-Host "Created $zip"
Write-Host "SHA-256: $hash"
