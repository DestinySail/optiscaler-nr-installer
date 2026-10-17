param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$Repo = 'DestinySail/optiscaler-nr-installer',
    [string]$MinisignKey = ''
)

$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$version = $Version.TrimStart('v')
$repoRoot = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $repoRoot 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$namedExe = Join-Path $dist 'OptiScalerNR.exe'
Copy-Item -LiteralPath $exePath -Destination $namedExe -Force
$hash = (Get-FileHash -LiteralPath $namedExe -Algorithm SHA256).Hash.ToLowerInvariant()
$url = "https://github.com/$Repo/releases/download/app-v$version/OptiScalerNR.exe"
$manifest = [ordered]@{
    schema = 1
    version = $version
    platform = 'windows-x64'
    exe_url = $url
    sha256 = $hash
    notes = "OptiScaler NR Installer $version"
}
$manifestPath = Join-Path $dist 'update-manifest.json'
$manifest | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $manifestPath -Encoding utf8
$signaturePath = Join-Path $dist 'update-manifest.json.minisig'

if ($MinisignKey) {
    if (-not (Get-Command minisign -ErrorAction SilentlyContinue)) {
        throw 'minisign 不在 PATH 中'
    }
    & minisign -S -s $MinisignKey -m $manifestPath -x $signaturePath
    if ($LASTEXITCODE -ne 0) {
        throw 'minisign 签名失败'
    }
}
else {
    Write-Warning '未提供 MinisignKey：可以构建 Release 资产，但不能用于程序自更新。'
}

Write-Host "Version: $version"
Write-Host "EXE SHA-256: $hash"
Write-Host "Manifest: $manifestPath"
if (Test-Path -LiteralPath $signaturePath) {
    Write-Host "Signature: $signaturePath"
}
