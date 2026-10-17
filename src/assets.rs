use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::{extract_zip, find_extras_root, find_release_root, read_small_text};
use crate::model::{AppPaths, Progress, Settings, UpstreamRelease};
use crate::updates::{check_upstream, download_verified, load_cached_upstream};
use crate::util::{format_bytes, sha256_file};

#[derive(Debug, Clone)]
pub struct PreparedComponents {
    pub release_root: PathBuf,
    pub extras_root: PathBuf,
    pub upstream_tag: String,
    pub extras_version: String,
}

pub fn prepare(
    paths: &AppPaths,
    settings: &Settings,
    upstream: Option<UpstreamRelease>,
    mut progress: impl FnMut(Progress),
) -> Result<PreparedComponents, String> {
    let upstream = if settings.local_upstream_source.is_some() {
        None
    } else {
        Some(match upstream.or_else(|| load_cached_upstream(paths)) {
            Some(release) => release,
            None => check_upstream(paths)?,
        })
    };
    let (release_input, upstream_tag, release_key) =
        if let Some(source) = settings.local_upstream_source.as_deref() {
            progress(Progress::new(
                "准备组件",
                format!("使用本地主包: {}", source.display()),
                None,
            ));
            let fingerprint = source_fingerprint(source)?;
            (
                source.to_path_buf(),
                "local".to_string(),
                format!("local-{}", &fingerprint[..fingerprint.len().min(16)]),
            )
        } else {
            let upstream = upstream
                .as_ref()
                .ok_or_else(|| "缺少上游发布信息".to_string())?;
            let destination = paths
                .cache
                .join("upstream")
                .join(upstream.asset.id.to_string())
                .join(&upstream.asset.name);
            progress(Progress::new(
                "准备组件",
                format!(
                    "下载 {} · {}",
                    upstream.tag,
                    format_bytes(upstream.asset.size)
                ),
                Some(0.0),
            ));
            download_verified(
                &upstream.asset.url,
                &destination,
                &upstream.asset.sha256,
                Some(upstream.asset.size),
                "下载主组件",
                &mut progress,
            )?;
            (
                destination,
                upstream.tag.clone(),
                format!("{}-{}", upstream.asset.id, &upstream.asset.sha256[..12]),
            )
        };
    let release_root = materialize(
        paths,
        &release_input,
        "upstream",
        &release_key,
        "解压主组件",
        &mut progress,
    )?;
    let release_root = find_release_root(&release_root)?;
    validate_release(&release_root)?;
    let actual_version = read_small_text(&release_root.join("VERSION"), 1024)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let upstream_tag = actual_version.unwrap_or(upstream_tag);

    let (extras_input, extras_version, extras_key) =
        if let Some(source) = settings.local_extras_source.as_deref() {
            progress(Progress::new(
                "准备组件",
                format!("使用本地固定资源: {}", source.display()),
                None,
            ));
            let fingerprint = source_fingerprint(source)?;
            (
                source.to_path_buf(),
                "local".to_string(),
                format!("local-{}", &fingerprint[..fingerprint.len().min(16)]),
            )
        } else if let Some(extras) = settings.extras.as_ref() {
            if extras.url.trim().is_empty() || extras.sha256.trim().is_empty() {
                return Err("固定资源设置缺少 URL 或 SHA-256".to_string());
            }
            let destination = paths
                .cache
                .join("extras")
                .join(format!("{}.zip", safe_label(&extras.version)));
            progress(Progress::new(
                "准备组件",
                format!("下载固定资源 {}", extras.version),
                Some(0.0),
            ));
            download_verified(
                &extras.url,
                &destination,
                &extras.sha256,
                None,
                "下载固定资源",
                &mut progress,
            )?;
            (
                destination,
                extras.version.clone(),
                format!(
                    "{}-{}",
                    safe_label(&extras.version),
                    &extras.sha256[..extras.sha256.len().min(12)]
                ),
            )
        } else {
            return Err(
            "未配置固定资源包。请在维护页填写 extras-v1.zip 的 URL 和 SHA-256，或导入本地资源包。"
                .to_string(),
        );
        };
    let extras_root = materialize(
        paths,
        &extras_input,
        "extras",
        &extras_key,
        "解压固定资源",
        &mut progress,
    )?;
    let extras_root = find_extras_root(&extras_root)?;
    validate_extras(&extras_root)?;

    Ok(PreparedComponents {
        release_root,
        extras_root,
        upstream_tag,
        extras_version,
    })
}

fn materialize(
    paths: &AppPaths,
    source: &Path,
    kind: &str,
    suffix: &str,
    progress_name: &str,
    progress: &mut impl FnMut(Progress),
) -> Result<PathBuf, String> {
    if source.is_dir() {
        return Ok(source.to_path_buf());
    }
    if !source.is_file() {
        return Err(format!("组件来源不存在: {}", source.display()));
    }
    let destination = paths.staging.join(format!("{kind}-{suffix}"));
    if destination.join(".__complete").is_file() {
        return Ok(destination);
    }
    let destination = paths
        .staging
        .join(format!("{kind}-{suffix}-{}", crate::util::now_ms()));
    fs::create_dir_all(&destination).map_err(|e| format!("创建 staging 失败: {e}"))?;
    extract_zip(source, &destination, progress_name, &mut *progress)?;
    crate::util::write_atomic(&destination.join(".__complete"), b"ok")?;
    Ok(destination)
}

fn validate_release(root: &Path) -> Result<(), String> {
    let required = [
        "OptiScaler.dll",
        "OptiScaler.ini",
        "OptiScaler",
        "MochizukiNrRuntime.dll",
        "dlssnr-amd",
        "person-model",
    ];
    for item in required {
        if !root.join(item).exists() {
            return Err(format!("主组件缺少 {item}: {}", root.display()));
        }
    }
    Ok(())
}

fn validate_extras(root: &Path) -> Result<(), String> {
    for item in [
        "MFGUnlocker.asi",
        "dlssnr-amd/dlssnr.bin",
        "dlssnr-amd/dlssnr-int4.bin",
    ] {
        if !root.join(item).is_file() {
            return Err(format!("固定资源缺少 {item}: {}", root.display()));
        }
    }
    Ok(())
}

fn safe_label(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn source_fingerprint(source: &Path) -> Result<String, String> {
    if source.is_file() {
        sha256_file(source)
    } else {
        Ok(format!("dir:{}", source.display()))
    }
}
