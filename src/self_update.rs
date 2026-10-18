use std::fs;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::model::{APP_VERSION, AppPaths, Progress, Settings};
use crate::updates::download_verified;

#[derive(Debug, Clone)]
pub struct AppUpdateInfo {
    pub version: String,
    pub exe_url: String,
    pub sha256: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    schema: u32,
    version: String,
    platform: String,
    exe_url: String,
    sha256: String,
    #[serde(default)]
    notes: String,
}

pub fn check(paths: &AppPaths, settings: &Settings) -> Result<Option<AppUpdateInfo>, String> {
    if settings.release_repo.trim().is_empty() {
        return Ok(None);
    }
    available(&crate::catalog::check(paths, settings, false)?.installer)
}

pub fn available(manifest: &UpdateManifest) -> Result<Option<AppUpdateInfo>, String> {
    let current = Version::parse(APP_VERSION).map_err(|e| format!("当前程序版本无效: {e}"))?;
    let version = Version::parse(&manifest.version).map_err(|e| format!("更新版本无效: {e}"))?;
    if version <= current {
        return Ok(None);
    }
    Ok(Some(AppUpdateInfo {
        version: manifest.version.clone(),
        exe_url: manifest.exe_url.clone(),
        sha256: manifest.sha256.clone(),
        notes: manifest.notes.clone(),
    }))
}

#[cfg(test)]
fn verify_manifest(
    repo: &str,
    manifest_bytes: &[u8],
    signature_text: &str,
    public_key: &str,
    expected_version: &str,
) -> Result<Option<AppUpdateInfo>, String> {
    use minisign_verify::{PublicKey, Signature};
    let signature =
        Signature::decode(signature_text).map_err(|e| format!("更新签名格式无效: {e}"))?;
    let key = PublicKey::from_base64(public_key).map_err(|e| format!("内置更新公钥无效: {e}"))?;
    key.verify(manifest_bytes, &signature, false)
        .map_err(|e| format!("更新清单验签失败: {e}"))?;
    let manifest = serde_json::from_slice::<UpdateManifest>(manifest_bytes)
        .map_err(|e| format!("更新清单 JSON 无效: {e}"))?;
    if manifest.version != expected_version {
        return Err("更新清单版本与 Release tag 不一致".to_string());
    }
    validate_manifest(repo, &manifest)?;
    available(&manifest)
}

pub fn validate_manifest(repo: &str, manifest: &UpdateManifest) -> Result<(), String> {
    let version = Version::parse(&manifest.version).map_err(|_| "更新清单版本无效")?;
    if manifest.schema != 1 || manifest.platform != "windows-x64" || !version.pre.is_empty() {
        return Err("更新清单平台、版本或格式不受支持".to_string());
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("更新清单 SHA-256 无效".to_string());
    }
    let allowed_prefix = format!("https://github.com/{repo}/releases/download/");
    if !manifest.exe_url.starts_with(&allowed_prefix) {
        return Err("更新清单下载地址不属于配置的发布仓库".to_string());
    }
    Ok(())
}

pub fn apply(
    paths: &AppPaths,
    update: &AppUpdateInfo,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    let filename = format!("OptiScalerNR-{}.exe", update.version);
    let downloaded = paths.updates.join(&filename);
    download_verified(
        &update.exe_url,
        &downloaded,
        &update.sha256,
        None,
        "下载程序更新",
        &mut progress,
    )?;
    progress(Progress::new("安装更新", "正在替换安装器", Some(1.0)));
    self_replace::self_replace(&downloaded).map_err(|e| format!("替换当前 EXE 失败: {e}"))?;
    let _ = fs::remove_file(&downloaded);
    Ok(())
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
            "0.3.0",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.version, "0.3.0");
        assert!(
            verify_manifest(
                "DestinySail/optiscaler-nr-installer",
                manifest,
                signature,
                TEST_PUBLIC_KEY,
                "0.4.0",
            )
            .is_err()
        );
        assert!(
            verify_manifest(
                "someone/else",
                manifest,
                signature,
                TEST_PUBLIC_KEY,
                "0.3.0",
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
                "0.3.0",
            )
            .is_err()
        );
    }
}
