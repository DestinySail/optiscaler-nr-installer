use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::model::{
    AppPaths, Progress, UPSTREAM_REPO, UpstreamAsset, UpstreamCheckCache, UpstreamRelease,
};
use crate::util::{now_ms, read_json, safe_relative, sha256_file, write_json_atomic};

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    id: u64,
    tag_name: String,
    published_at: Option<String>,
    prerelease: bool,
    draft: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    id: u64,
    name: String,
    size: u64,
    digest: Option<String>,
    updated_at: String,
    browser_download_url: String,
}

pub fn check_upstream(paths: &AppPaths) -> Result<UpstreamRelease, String> {
    let cached = read_json::<UpstreamCheckCache>(&paths.upstream_check).ok();
    let client = http_client()?;
    let mut request = client
        .get(format!(
            "https://api.github.com/repos/{UPSTREAM_REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "OptiScalerNR-Installer");
    if let Some(etag) = cached.as_ref().and_then(|cache| cache.etag.as_deref()) {
        request = request.header("If-None-Match", etag);
    }
    let response = request
        .send()
        .map_err(|e| format!("检查上游发布失败: {e}"))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return cached
            .map(|cache| cache.latest)
            .ok_or_else(|| "上游返回 304，但没有可用缓存".to_string());
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!(
            "检查上游发布失败: HTTP {status}{}",
            if body.is_empty() {
                String::new()
            } else {
                format!(" - {}", body.chars().take(200).collect::<String>())
            }
        ));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let release = response
        .json::<GitHubRelease>()
        .map_err(|e| format!("解析上游发布失败: {e}"))?;
    if release.prerelease || release.draft {
        return Err("GitHub latest 返回了预发布或草稿版本，拒绝自动使用".to_string());
    }
    let asset = release
        .assets
        .into_iter()
        .filter(|asset| {
            asset.name.starts_with("OptScaler-NR-")
                && asset.name.to_ascii_lowercase().ends_with(".zip")
        })
        .max_by_key(|asset| asset.id)
        .ok_or_else(|| "上游最新发布没有 OptScaler-NR-*.zip".to_string())?;
    let safe_name = safe_relative(std::path::Path::new(&asset.name))?;
    if safe_name.components().count() != 1 {
        return Err("上游资产名称不是单个文件名，拒绝使用".to_string());
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| "上游资产没有 GitHub SHA-256 digest，拒绝安装".to_string())?
        .to_ascii_lowercase();
    let latest = UpstreamRelease {
        release_id: release.id,
        tag: release.tag_name,
        published_at: release.published_at.unwrap_or_default(),
        asset: UpstreamAsset {
            id: asset.id,
            name: safe_name.to_string_lossy().to_string(),
            size: asset.size,
            sha256,
            url: asset.browser_download_url,
            updated_at: asset.updated_at,
        },
        checked_at_ms: now_ms(),
    };
    write_json_atomic(
        &paths.upstream_check,
        &UpstreamCheckCache {
            latest: latest.clone(),
            etag,
        },
    )?;
    Ok(latest)
}

pub fn load_cached_upstream(paths: &AppPaths) -> Option<UpstreamRelease> {
    read_json::<UpstreamCheckCache>(&paths.upstream_check)
        .ok()
        .map(|cache| cache.latest)
}

pub fn download_verified(
    url: &str,
    destination: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    progress_name: &str,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    if destination.is_file() && sha256_file(destination)?.eq_ignore_ascii_case(expected_sha256) {
        progress(Progress::new(progress_name, "已命中校验缓存", Some(1.0)));
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建下载目录失败: {e}"))?;
    }
    let part = destination.with_extension("part");
    let mut last_error = String::new();
    for attempt in 1..=3 {
        match download_attempt(
            url,
            &part,
            expected_sha256,
            expected_size,
            progress_name,
            &mut progress,
        ) {
            Ok(()) => {
                if destination.exists() {
                    fs::remove_file(destination).map_err(|e| format!("替换旧下载文件失败: {e}"))?;
                }
                fs::rename(&part, destination).map_err(|e| format!("提交下载文件失败: {e}"))?;
                return Ok(());
            }
            Err(error) => {
                last_error = error;
                if attempt < 3 {
                    progress(Progress::new(
                        progress_name,
                        format!("下载失败，准备第 {} 次重试", attempt + 1),
                        None,
                    ));
                    std::thread::sleep(Duration::from_secs(attempt as u64));
                }
            }
        }
    }
    Err(format!("下载失败（已重试 3 次）: {last_error}"))
}

fn download_attempt(
    url: &str,
    part: &Path,
    expected_sha256: &str,
    expected_size: Option<u64>,
    progress_name: &str,
    progress: &mut impl FnMut(Progress),
) -> Result<(), String> {
    let existing = part.metadata().map(|m| m.len()).unwrap_or(0);
    let client = http_client()?;
    let mut request = client
        .get(url)
        .header("User-Agent", "OptiScalerNR-Installer");
    if existing > 0 {
        request = request.header("Range", format!("bytes={existing}-"));
    }
    let mut response = request.send().map_err(|e| format!("网络请求失败: {e}"))?;
    if existing > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        let _ = fs::remove_file(part);
        return Err("服务器未接受断点续传，请重试".to_string());
    }
    response
        .error_for_status_ref()
        .map_err(|e| format!("下载请求失败: {e}"))?;
    let total = response
        .content_length()
        .map(|remaining| remaining + existing);
    if let (Some(total), Some(expected)) = (total, expected_size)
        && total != expected
    {
        let _ = fs::remove_file(part);
        return Err(format!("下载大小不符：远端 {total}，计划 {expected}"));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(|e| format!("打开断点文件失败: {e}"))?;
    let mut done = existing;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|e| format!("下载中断: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|e| format!("写入下载文件失败: {e}"))?;
        done += read as u64;
        progress(Progress::new(
            progress_name,
            format!(
                "已下载 {} / {}",
                crate::util::format_bytes(done),
                total
                    .map(crate::util::format_bytes)
                    .unwrap_or_else(|| "?".to_string())
            ),
            total.map(|total| (done as f32 / total.max(1) as f32).clamp(0.0, 1.0)),
        ));
    }
    file.flush().map_err(|e| format!("刷新下载文件失败: {e}"))?;
    let actual = sha256_file(part)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(part);
        return Err("下载文件 SHA-256 不匹配，已删除".to_string());
    }
    if let Some(expected) = expected_size {
        let actual_size = part.metadata().map(|m| m.len()).unwrap_or(0);
        if actual_size != expected {
            let _ = fs::remove_file(part);
            return Err(format!(
                "下载文件大小不符：实际 {actual_size}，期望 {expected}"
            ));
        }
    }
    Ok(())
}

pub fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(3600))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}
