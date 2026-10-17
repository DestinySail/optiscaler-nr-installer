param(
    [string]$Repo = 'DestinySail/optiscaler-nr-installer',
    [Parameter(Mandatory = $true)][string]$MinisignKey,
    [string]$MinisignExe = 'minisign',
    [string]$OutputDir = 'dist/update-feed'
)

$ErrorActionPreference = 'Stop'
if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Invalid repository' }
$publicKeyFile = Join-Path (Split-Path -Parent $PSScriptRoot) 'release/update-key.pub'
$publicKey = (Get-Content -LiteralPath $publicKeyFile | Where-Object { $_ -match '^RW' }).Trim()
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

function Read-GitHubJson([string]$Path) {
    $result = & gh api $Path --header 'X-GitHub-Api-Version: 2022-11-28'
    if ($LASTEXITCODE -ne 0) { throw "GitHub metadata request failed: $Path" }
    return ($result | ConvertFrom-Json)
}

function Save-ReleaseText($Release, [string]$Name, [string]$Destination) {
    $asset = @($Release.assets | Where-Object name -EQ $Name)
    if ($asset.Count -ne 1) { throw "Release must contain exactly one $Name" }
    if ($asset[0].size -gt 131072) { throw 'Release metadata exceeds size limit' }
    $prefix = "https://github.com/$Repo/releases/download/$($Release.tag_name)/"
    if (-not $asset[0].browser_download_url.StartsWith($prefix)) { throw 'Invalid metadata URL' }
    Invoke-WebRequest -Uri $asset[0].browser_download_url -OutFile $Destination -TimeoutSec 30
}

# Only this maintainer-side job queries Releases; clients consume the signed static file.
$pages = & gh api "repos/$Repo/releases?per_page=100" --paginate --slurp
if ($LASTEXITCODE -ne 0) { throw 'Failed to list installer releases' }
$releases = @($pages | ConvertFrom-Json | ForEach-Object { $_ } | ForEach-Object { $_ })
$release = $releases | Where-Object {
    -not $_.draft -and -not $_.prerelease -and $_.tag_name -match '^app-v\d+\.\d+\.\d+$'
} | Sort-Object { [version]$_.tag_name.Substring(5) } -Descending | Select-Object -First 1
if (-not $release) { throw 'No stable installer release' }

$manifestPath = Join-Path $OutputDir 'installer-manifest.json'
$signaturePath = "$manifestPath.minisig"
Save-ReleaseText $release 'update-manifest.json' $manifestPath
Save-ReleaseText $release 'update-manifest.json.minisig' $signaturePath
& $MinisignExe -V -P $publicKey -m $manifestPath -x $signaturePath
if ($LASTEXITCODE -ne 0) { throw 'Installer manifest signature verification failed' }
$installer = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($installer.schema -ne 1 -or $installer.platform -ne 'windows-x64' -or
    $installer.version -ne $release.tag_name.Substring(5) -or
    $installer.sha256 -notmatch '^[a-fA-F0-9]{64}$' -or
    -not $installer.exe_url.StartsWith("https://github.com/$Repo/releases/download/$($release.tag_name)/")) {
    throw 'Invalid installer manifest'
}
$exe = @($release.assets | Where-Object browser_download_url -EQ $installer.exe_url)
if ($exe.Count -ne 1 -or ($exe[0].digest -and $exe[0].digest -ne "sha256:$($installer.sha256)")) {
    throw 'Installer asset is missing or its digest disagrees with the signed manifest'
}

$upstream = Read-GitHubJson 'repos/TheAutomatic/dlss-5-amd-project/releases/latest'
if ($upstream.draft -or $upstream.prerelease -or $upstream.tag_name -notmatch '^v?\d+\.\d+\.\d+$') {
    throw 'Upstream release is not stable'
}
$asset = $upstream.assets | Where-Object { $_.name -match '^OptScaler-NR-[^/\\:]+\.zip$' } |
    Sort-Object id -Descending | Select-Object -First 1
if (-not $asset -or $asset.digest -notmatch '^sha256:[a-fA-F0-9]{64}$' -or $asset.size -le 0 -or
    -not $asset.browser_download_url.StartsWith('https://github.com/TheAutomatic/dlss-5-amd-project/releases/download/')) {
    throw 'Upstream package has no valid source, size or SHA-256 digest'
}
$catalog = [ordered]@{
    schema = 1
    installer = $installer
    upstream = [ordered]@{
        release_id = $upstream.id
        tag = $upstream.tag_name
        published_at = $upstream.published_at
        asset = [ordered]@{
            id = $asset.id
            name = $asset.name
            size = $asset.size
            sha256 = $asset.digest.Substring(7).ToLowerInvariant()
            url = $asset.browser_download_url
            updated_at = $asset.updated_at
        }
        checked_at_ms = 0
    }
}
$catalogPath = Join-Path $OutputDir 'update-catalog.json'
$text = ($catalog | ConvertTo-Json -Depth 8) + "`n"
$unchanged = (Test-Path -LiteralPath $catalogPath) -and
    ([IO.File]::ReadAllText([IO.Path]::GetFullPath($catalogPath)) -eq $text) -and
    (Test-Path -LiteralPath "$catalogPath.minisig")
if (-not $unchanged) {
    $candidate = Join-Path $OutputDir 'candidate.json'
    [IO.File]::WriteAllText([IO.Path]::GetFullPath($candidate), $text, [Text.UTF8Encoding]::new($false))
    & $MinisignExe -S -s $MinisignKey -m $candidate -x "$candidate.minisig" -t 'OptiScalerNR update catalog'
    if ($LASTEXITCODE -ne 0) { throw 'Catalog signing failed' }
    & $MinisignExe -V -P $publicKey -m $candidate -x "$candidate.minisig"
    if ($LASTEXITCODE -ne 0) { throw 'Catalog signature verification failed' }
    Move-Item -LiteralPath $candidate -Destination $catalogPath -Force
    Move-Item -LiteralPath "$candidate.minisig" -Destination "$catalogPath.minisig" -Force
}
& $MinisignExe -V -P $publicKey -m $catalogPath -x "$catalogPath.minisig"
if ($LASTEXITCODE -ne 0) { throw 'Published catalog signature verification failed' }
Write-Host "Catalog ready: installer $($installer.version), upstream $($upstream.tag_name)"
