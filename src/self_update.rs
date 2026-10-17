use std::fs;
use std::path::PathBuf;

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::model::{APP_VERSION, AppPaths, Settings};
use crate::updates::{download_verified, http_client};
use crate::util::{sha256_file, write_json_atomic};

#[derive(Debug, Clone)]
pub struct AppUpdateInfo {
    pub version: String,
    pub exe_url: String,
    pub sha256: String,
    pub notes: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateManifest {
    schema: u32,
    version: String,
    platform: String,
    exe_url: String,
    sha256: String,
    #[serde(default)]
    notes: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

pub fn check(settings: &Settings) -> Result<Option<AppUpdateInfo>, String> {
    let repo = settings.release_repo.trim();
    if repo.is_empty() {
        return Ok(None);
    }
    let client = http_client()?;
    let releases = client
        .get(format!(
            "https://api.github.com/repos/{repo}/releases?per_page=30"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "OptiScalerNR-Installer")
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|e| format!("检查程序更新失败: {e}"))?
        .json::<Vec<Release>>()
        .map_err(|e| format!("解析程序更新列表失败: {e}"))?;
    let current = Version::parse(APP_VERSION).map_err(|e| format!("当前程序版本无效: {e}"))?;
    let newest = releases
        .into_iter()
        .filter(|release| !release.prerelease && !release.draft)
        .filter_map(|release| {
            let version = release.tag_name.strip_prefix("app-v")?;
            let parsed = Version::parse(version).ok()?;
            (parsed > current).then_some((parsed, release))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, release)) = newest else {
        return Ok(None);
    };
    let public_key = option_env!("NR_UPDATE_PUBKEY")
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            "发现程序新版本，但此构建没有内置更新公钥；发布构建需设置 NR_UPDATE_PUBKEY".to_string()
        })?;
    let manifest_asset = find_asset(&release.assets, "update-manifest.json")?;
    let signature_asset = find_asset(&release.assets, "update-manifest.json.minisig")?;
    let manifest_bytes = fetch_small(&client, &manifest_asset.browser_download_url, 64 * 1024)?;
    let signature_text = String::from_utf8(fetch_small(
        &client,
        &signature_asset.browser_download_url,
        16 * 1024,
    )?)
    .map_err(|e| format!("更新签名不是 UTF-8: {e}"))?;
    verify_manifest(
        repo,
        &manifest_bytes,
        &signature_text,
        public_key,
        &version.to_string(),
    )
}

fn verify_manifest(
    repo: &str,
    manifest_bytes: &[u8],
    signature_text: &str,
    public_key: &str,
    expected_version: &str,
) -> Result<Option<AppUpdateInfo>, String> {
    let signature =
        Signature::decode(signature_text).map_err(|e| format!("更新签名格式无效: {e}"))?;
    let key = PublicKey::from_base64(public_key).map_err(|e| format!("内置更新公钥无效: {e}"))?;
    key.verify(manifest_bytes, &signature, false)
        .map_err(|e| format!("更新清单验签失败: {e}"))?;
    let manifest = serde_json::from_slice::<UpdateManifest>(manifest_bytes)
        .map_err(|e| format!("更新清单 JSON 无效: {e}"))?;
    if manifest.schema != 1 || manifest.platform != "windows-x64" {
        return Err("更新清单平台或格式不受支持".to_string());
    }
    if manifest.version != expected_version {
        return Err("更新清单版本与 Release tag 不一致".to_string());
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("更新清单 SHA-256 无效".to_string());
    }
    let allowed_prefix = format!("https://github.com/{repo}/releases/download/");
    if !manifest.exe_url.starts_with(&allowed_prefix) {
        return Err("更新清单下载地址不属于配置的发布仓库".to_string());
    }
    Ok(Some(AppUpdateInfo {
        version: manifest.version,
        exe_url: manifest.exe_url,
        sha256: manifest.sha256,
        notes: manifest.notes,
    }))
}

pub fn apply(paths: &AppPaths, update: &AppUpdateInfo) -> Result<PathBuf, String> {
    let filename = format!("OptiScalerNR-{}.exe", update.version);
    let downloaded = paths.updates.join(&filename);
    download_verified(
        &update.exe_url,
        &downloaded,
        &update.sha256,
        None,
        "下载程序更新",
        |_| {},
    )?;
    let current = std::env::current_exe().map_err(|e| format!("读取当前 EXE 失败: {e}"))?;
    let backup = paths.updates.join(format!("previous-{APP_VERSION}.exe"));
    if !backup.exists() {
        fs::copy(&current, &backup).map_err(|e| format!("备份当前 EXE 失败: {e}"))?;
        write_json_atomic(
            &paths.updates.join("previous.json"),
            &serde_json::json!({
                "version": APP_VERSION,
                "path": backup,
                "sha256": sha256_file(&backup)?,
            }),
        )?;
    }
    self_replace::self_replace(&downloaded).map_err(|e| format!("替换当前 EXE 失败: {e}"))?;
    Ok(backup)
}

fn find_asset<'a>(assets: &'a [ReleaseAsset], name: &str) -> Result<&'a ReleaseAsset, String> {
    assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| format!("程序 Release 缺少 {name}"))
}

fn fetch_small(
    client: &reqwest::blocking::Client,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let bytes = client
        .get(url)
        .header("User-Agent", "OptiScalerNR-Installer")
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.bytes())
        .map_err(|e| format!("下载更新清单失败: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("更新清单超过 {limit} 字节限制"));
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "RWRZRk4J8IpdBVApDDKTOdBHzSt0ctpCowVtK54cwnNkzMh5S6dyHCTO";

    #[test]
    fn verifies_signed_manifest_and_rejects_tampering() {
        let manifest = include_bytes!("../tests/fixtures/update-manifest.json");
        let signature = include_str!("../tests/fixtures/update-manifest.json.minisig");
        let parsed = verify_manifest(
            "DestinySail/optiscaler-nr-installer",
            manifest,
            signature,
            TEST_PUBLIC_KEY,
            "0.2.0",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.version, "0.2.0");
        assert!(
            verify_manifest(
                "DestinySail/optiscaler-nr-installer",
                manifest,
                signature,
                TEST_PUBLIC_KEY,
                "0.3.0",
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                "someone/else",
                manifest,
                signature,
                TEST_PUBLIC_KEY,
                "0.2.0",
            )
            .is_err()
        );

        let mut tampered = manifest.to_vec();
        tampered[10] ^= 1;
        assert!(
            verify_manifest(
                "DestinySail/optiscaler-nr-installer",
                &tampered,
                signature,
                TEST_PUBLIC_KEY,
                "0.2.0",
            )
            .is_err()
        );
    }
}
