use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::{AppPaths, Progress, Settings, UpstreamRelease};
use crate::util::{safe_relative, sha256_file};

pub fn check_upstream(paths: &AppPaths, settings: &Settings) -> Result<UpstreamRelease, String> {
    let catalog = crate::catalog::check(paths, settings, false)?;
    catalog.upstream.ok_or_else(|| {
        catalog
            .upstream_error
            .unwrap_or_else(|| "更新清单未包含主包信息".into())
    })
}

pub fn load_cached_upstream(paths: &AppPaths, settings: &Settings) -> Option<UpstreamRelease> {
    crate::catalog::cached(paths, settings).and_then(|catalog| catalog.upstream)
}

pub fn upstream_archive(paths: &AppPaths, release: &UpstreamRelease) -> PathBuf {
    paths
        .cache
        .join("upstream")
        .join(release.asset.id.to_string())
        .join(&release.asset.name)
}

pub fn download_upstream(
    paths: &AppPaths,
    release: &UpstreamRelease,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let name = safe_relative(Path::new(&release.asset.name))?;
    if name.components().count() != 1
        || !release.asset.name.starts_with("OptScaler-NR-")
        || !release.asset.name.to_ascii_lowercase().ends_with(".zip")
    {
        return Err("上游主包缓存名称无效，拒绝下载和清理".into());
    }
    let destination = upstream_archive(paths, release);
    download_verified(
        &release.asset.url,
        &destination,
        &release.asset.sha256,
        Some(release.asset.size),
        "下载主组件",
        &mut progress,
    )?;
    prune_upstream_cache(paths, release.asset.id)
        .map_err(|error| format!("新版主包已校验，但旧缓存清理失败：{error}"))?;
    Ok(destination)
}

fn prune_upstream_cache(paths: &AppPaths, keep_id: u64) -> Result<(), String> {
    let cache = paths
        .cache
        .canonicalize()
        .map_err(|e| format!("读取缓存目录失败：{e}"))?;
    let data_root = paths
        .root
        .canonicalize()
        .map_err(|e| format!("读取数据目录失败：{e}"))?;
    if cache != data_root.join("cache") {
        return Err("缓存目录存在重定向，拒绝自动清理".into());
    }
    let root = paths
        .cache
        .join("upstream")
        .canonicalize()
        .map_err(|e| format!("读取主包缓存目录失败：{e}"))?;
    let keep = root.join(keep_id.to_string());
    if root != cache.join("upstream") || keep.canonicalize().ok().as_ref() != Some(&keep) {
        return Err("主包缓存目录存在重定向，拒绝自动清理".into());
    }
    // Only generated asset-ID directories belong to the upstream package cache.
    for entry in fs::read_dir(&root).map_err(|e| format!("读取主包缓存失败：{e}"))? {
        let entry = entry.map_err(|e| format!("读取主包缓存项失败：{e}"))?;
        let name = entry.file_name();
        let managed_id = name
            .to_str()
            .and_then(|name| name.parse::<u64>().ok())
            .is_some_and(|id| name == id.to_string().as_str());
        if managed_id
            && entry.path() != keep
            && entry
                .file_type()
                .map_err(|e| format!("检查主包缓存项失败：{e}"))?
                .is_dir()
        {
            crate::util::remove_tree_safely(&entry.path())?;
        }
    }
    Ok(())
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

// ponytail: "system-proxy" 让 reqwest 读注册表的 ProxyEnable/ProxyServer，走用户代理。
// 已知上限：ProxyServer 若为 "http=host:port;https=host:port" 分协议格式，reqwest 解析失败会静默直连；
// 升级路径是自己解析该格式再 .proxy()，目前统计到的代理软件都写单一 host:port。
pub fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(3600))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::now_ms;
    use std::io::BufRead;
    use std::net::TcpListener;

    fn serve(response: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            for line in std::io::BufReader::new(&stream).lines() {
                let line = line.unwrap();
                if line.is_empty() {
                    break;
                }
                request.push_str(&line);
                request.push('\n');
            }
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (url, worker)
    }

    #[test]
    fn upstream_checks_downloads_and_safe_cache_retention() {
        let root = std::env::temp_dir().join(format!(
            "optinr-update-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir(&root).unwrap();
        let paths = AppPaths {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            root: root.clone(),
        };
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/update-catalog.json")).unwrap();
        let latest: UpstreamRelease = serde_json::from_value(value["upstream"].clone()).unwrap();
        let body = "verified upstream package";
        let (url, worker) = serve(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        let mut release = latest;
        release.asset.url = url;
        release.asset.size = body.len() as u64;
        release.asset.sha256 = {
            use sha2::Digest;
            crate::util::hex_lower(&sha2::Sha256::digest(body.as_bytes()))
        };
        let old_dir = paths.cache.join("upstream/1");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(
            old_dir.join("OptScaler-NR-old.zip"),
            b"old verified package",
        )
        .unwrap();
        fs::write(
            old_dir.join("OptScaler-NR-old.part"),
            b"old partial download",
        )
        .unwrap();
        let untouched = [
            paths.cache.join("extras/keep.zip"),
            paths.cache.join("upstream/manual/keep.zip"),
            paths.cache.join("upstream/1234"),
            paths.games.join("game/keep.dll"),
        ];
        for path in &untouched {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"untouched").unwrap();
        }
        let destination = upstream_archive(&paths, &release);
        let mut failed_release = release.clone();
        failed_release.asset.url = "http://127.0.0.1:0/unavailable".into();
        assert!(download_upstream(&paths, &failed_release, |_| {}).is_err());
        assert_eq!(
            fs::read(old_dir.join("OptScaler-NR-old.zip")).unwrap(),
            b"old verified package"
        );
        assert!(!destination.exists());
        failed_release.asset.name = "../outside.zip".into();
        assert!(download_upstream(&paths, &failed_release, |_| {}).is_err());
        assert!(old_dir.exists());
        let bad_body = "x".repeat(body.len());
        let (bad_url, bad_worker) = serve(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bad_body}",
            bad_body.len()
        ));
        assert!(
            download_attempt(
                &bad_url,
                &destination.with_extension("part"),
                &release.asset.sha256,
                Some(release.asset.size),
                "下载主组件",
                &mut |_| {}
            )
            .unwrap_err()
            .contains("SHA-256 不匹配")
        );
        bad_worker.join().unwrap();
        assert!(old_dir.join("OptScaler-NR-old.zip").is_file());
        assert!(!destination.exists());
        assert_eq!(
            download_upstream(&paths, &release, |_| {}).unwrap(),
            destination
        );
        worker.join().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), body.as_bytes());
        assert!(!old_dir.exists());
        fs::create_dir(&old_dir).unwrap();
        let old_file = old_dir.join("OptScaler-NR-old.zip");
        fs::write(&old_file, b"old verified package").unwrap();
        release.asset.url = "http://127.0.0.1:0/not-requested".into();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let locked = OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&old_file)
                .unwrap();
            assert!(
                download_upstream(&paths, &release, |_| {})
                    .unwrap_err()
                    .contains("新版主包已校验，但旧缓存清理失败")
            );
            assert_eq!(fs::read(&destination).unwrap(), body.as_bytes());
            drop(locked);
            assert_eq!(fs::read(&old_file).unwrap(), b"old verified package");
        }
        download_upstream(&paths, &release, |_| {}).unwrap();
        assert!(!old_dir.exists());
        assert_eq!(fs::read(&destination).unwrap(), body.as_bytes());
        for path in &untouched {
            assert_eq!(fs::read(path).unwrap(), b"untouched");
        }
        #[cfg(windows)]
        {
            let junction = |link: &Path, target: &Path| {
                let output = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(link.to_string_lossy().replace('/', "\\"))
                    .arg(target.to_string_lossy().replace('/', "\\"))
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            fs::create_dir(&old_dir).unwrap();
            junction(&old_dir.join("external-game"), &paths.games.join("game"));
            download_upstream(&paths, &release, |_| {}).unwrap();
            assert!(!old_dir.exists());
            assert_eq!(
                fs::read(paths.games.join("game/keep.dll")).unwrap(),
                b"untouched"
            );
            let upstream_root = paths.cache.join("upstream");
            let redirected = root.join("redirected");
            fs::rename(&upstream_root, &redirected).unwrap();
            junction(&upstream_root, &redirected);
            assert!(
                prune_upstream_cache(&paths, release.asset.id)
                    .unwrap_err()
                    .contains("重定向")
            );
            assert_eq!(
                fs::read(redirected.join("2").join(&release.asset.name)).unwrap(),
                body.as_bytes()
            );
            crate::util::remove_tree_safely(&upstream_root).unwrap();
            assert!(redirected.join("manual/keep.zip").is_file());
            fs::rename(&redirected, &upstream_root).unwrap();
            let redirected_cache = root.join("redirected-cache");
            fs::rename(&paths.cache, &redirected_cache).unwrap();
            junction(&paths.cache, &redirected_cache);
            assert!(
                prune_upstream_cache(&paths, release.asset.id)
                    .unwrap_err()
                    .contains("重定向")
            );
            crate::util::remove_tree_safely(&paths.cache).unwrap();
            assert!(redirected_cache.join("upstream/manual/keep.zip").is_file());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
