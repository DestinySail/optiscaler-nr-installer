use std::io::Read;
use std::time::Duration;

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::model::{AppPaths, Settings, UPSTREAM_REPO, UpstreamRelease};
use crate::self_update::{UpdateManifest, validate_manifest};
use crate::updates::http_client;
use crate::util::{now_ms, read_json, safe_relative, write_json_atomic};

const CHECK_INTERVAL_MS: u64 = 60 * 60 * 1000;
const RETRY_INTERVAL_MS: u64 = 5 * 60 * 1000;
const REFRAMEWORK_REPO: &str = "praydog/REFramework-nightly";
const PUBLIC_KEY: &str = match option_env!("NR_UPDATE_PUBKEY") {
    Some(key) if !key.is_empty() => key,
    _ => "RWTZaaq8Jlp7qRSoKK4tDYfpAslM5P/LTvHSSTox80Pzx0LDZUisoTgH",
};

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    schema: u32,
    pub installer: UpdateManifest,
    // ponytail: 主包与 REFramework 是外部发布，缺项或校验失败只降级对应组件，安装器更新不受影响。
    #[serde(default)]
    pub upstream: Option<UpstreamRelease>,
    #[serde(default)]
    pub reframework: Option<UpstreamRelease>,
    #[serde(skip)]
    pub warning: Option<String>,
    #[serde(skip)]
    pub upstream_error: Option<String>,
    #[serde(skip)]
    pub reframework_error: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    url: String,
    checked_at_ms: u64,
    retry_after_ms: u64,
    error: Option<String>,
    payload: Option<String>,
    signature: Option<String>,
}

fn catalog_url(settings: &Settings) -> Result<String, String> {
    let repo = settings.release_repo.trim();
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        return Err("更新仓库必须为 owner/repository".into());
    }
    Ok(format!(
        "https://raw.githubusercontent.com/{repo}/update-feed/update-catalog.json"
    ))
}

pub fn check(paths: &AppPaths, settings: &Settings, force: bool) -> Result<Catalog, String> {
    check_from(paths, settings, &catalog_url(settings)?, PUBLIC_KEY, force)
}

pub fn cached(paths: &AppPaths, settings: &Settings) -> Option<Catalog> {
    let cache = read_cache(paths, &catalog_url(settings).ok()?);
    verified_cache(&cache, settings, PUBLIC_KEY).ok()
}

fn read_cache(paths: &AppPaths, url: &str) -> Cache {
    read_json::<Cache>(&paths.updates.join("catalog-cache.json"))
        .ok()
        .filter(|cache| cache.url == url)
        .unwrap_or_else(|| Cache {
            url: url.to_string(),
            ..Default::default()
        })
}

fn verified_cache(cache: &Cache, settings: &Settings, public_key: &str) -> Result<Catalog, String> {
    let mut catalog = verify(
        cache
            .payload
            .as_deref()
            .ok_or("没有已验签的更新清单缓存")?
            .as_bytes(),
        cache.signature.as_deref().ok_or("更新清单缓存缺少签名")?,
        settings,
        public_key,
    )?;
    if let Some(upstream) = &mut catalog.upstream {
        upstream.checked_at_ms = cache.checked_at_ms;
    }
    catalog.warning = cache.error.clone();
    Ok(catalog)
}

fn check_from(
    paths: &AppPaths,
    settings: &Settings,
    url: &str,
    public_key: &str,
    force: bool,
) -> Result<Catalog, String> {
    let mut cache = read_cache(paths, url);
    let now = now_ms();
    if now < cache.retry_after_ms && cache.retry_after_ms - now <= RETRY_INTERVAL_MS {
        return verified_cache(&cache, settings, public_key).map_err(|_| {
            format!(
                "{}；更新检查冷却中，请稍后重试",
                cache.error.as_deref().unwrap_or("更新源暂不可用")
            )
        });
    }
    if !force
        && cache.error.is_none()
        && cache.checked_at_ms > 0
        && now >= cache.checked_at_ms
        && now.saturating_sub(cache.checked_at_ms) < CHECK_INTERVAL_MS
        && let Ok(catalog) = verified_cache(&cache, settings, public_key)
    {
        return Ok(catalog);
    }
    let fetched = (|| {
        let client = http_client()?;
        let payload = fetch_small(&client, url, 128 * 1024)?;
        let signature = fetch_small(&client, &format!("{url}.minisig"), 16 * 1024)?;
        verify(payload.as_bytes(), &signature, settings, public_key)?;
        Ok::<_, String>((payload, signature))
    })();
    match fetched {
        Ok((payload, signature)) => {
            cache.payload = Some(payload);
            cache.signature = Some(signature);
            cache.checked_at_ms = now;
            cache.retry_after_ms = 0;
            cache.error = None;
        }
        Err(error) => {
            cache.retry_after_ms = now.saturating_add(RETRY_INTERVAL_MS);
            cache.error = Some(error);
        }
    }
    write_json_atomic(&paths.updates.join("catalog-cache.json"), &cache)?;
    verified_cache(&cache, settings, public_key)
        .map_err(|_| cache.error.unwrap_or_else(|| "更新清单缓存无效".into()))
}

fn fetch_small(
    client: &reqwest::blocking::Client,
    url: &str,
    limit: u64,
) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", "OptiScalerNR-Installer")
        .timeout(Duration::from_secs(15))
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| format!("读取静态更新清单失败：{error}"))?;
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取更新清单失败：{error}"))?;
    if bytes.len() as u64 > limit {
        return Err("静态更新清单超过大小限制".into());
    }
    String::from_utf8(bytes).map_err(|_| "静态更新清单不是 UTF-8".into())
}

// ponytail: 上游 tag 不保证是 semver（出现过 1.13.3.3、日期串甚至无点版本），只按预发布标识拒绝。
// 版本大小完全由清单发布方决定；如需严格排序，改读 release 的 prerelease 字段。
fn is_stable_tag(tag: &str) -> bool {
    let tag = tag.trim().trim_start_matches(['v', 'V']);
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_+".contains(c))
        && ![
            "alpha", "beta", "pre", "rc", "nightly", "dev", "snapshot", "test", "canary", "draft",
            "preview",
        ]
        .iter()
        .any(|marker| tag.to_ascii_lowercase().contains(marker))
}

fn validate_upstream(upstream: &UpstreamRelease) -> Result<(), String> {
    if !is_stable_tag(&upstream.tag) {
        return Err(format!("主包版本 {} 带预发布标识，已跳过", upstream.tag));
    }
    let asset = &upstream.asset;
    let name = safe_relative(std::path::Path::new(&asset.name))?;
    if name.components().count() != 1
        || asset.name.contains(['/', '\\', ':'])
        || !asset.name.starts_with("OptScaler-NR-")
        || !asset.name.ends_with(".zip")
        || asset.id == 0
        || upstream.release_id == 0
        || asset.size == 0
        || asset.sha256.len() != 64
        || !asset.sha256.chars().all(|c| c.is_ascii_hexdigit())
        || !asset.url.starts_with(&format!(
            "https://github.com/{UPSTREAM_REPO}/releases/download/"
        ))
    {
        return Err("静态清单的主包来源、文件名或哈希无效".into());
    }
    Ok(())
}

fn validate_reframework(reframework: &UpstreamRelease) -> Result<(), String> {
    let asset = &reframework.asset;
    let name = safe_relative(std::path::Path::new(&asset.name))?;
    if name.components().count() != 1
        || asset.name != "REFramework.zip"
        || !reframework.tag.starts_with("nightly-")
        || reframework.release_id == 0
        || asset.id == 0
        || asset.size == 0
        || asset.sha256.len() != 64
        || !asset.sha256.chars().all(|c| c.is_ascii_hexdigit())
        || !asset.url.starts_with(&format!(
            "https://github.com/{REFRAMEWORK_REPO}/releases/download/"
        ))
    {
        return Err("静态清单的 REFramework 来源、文件名或哈希无效".into());
    }
    Ok(())
}

// 只对安装器清单做强制校验；上游组件的错误降级为组件级错误，不阻断安装器更新。
fn validate_components(catalog: &mut Catalog) {
    catalog.upstream_error = match &catalog.upstream {
        Some(upstream) => validate_upstream(upstream).err(),
        None => Some("更新清单未包含主包信息".into()),
    };
    if catalog.upstream_error.is_some() {
        catalog.upstream = None;
    }
    if let Some(reframework) = &catalog.reframework
        && let Err(error) = validate_reframework(reframework)
    {
        catalog.reframework_error = Some(error);
        catalog.reframework = None;
    }
}

fn verify(
    bytes: &[u8],
    signature: &str,
    settings: &Settings,
    public_key: &str,
) -> Result<Catalog, String> {
    let key =
        PublicKey::from_base64(public_key).map_err(|error| format!("更新公钥无效：{error}"))?;
    let signature =
        Signature::decode(signature).map_err(|error| format!("更新签名无效：{error}"))?;
    key.verify(bytes, &signature, false)
        .map_err(|error| format!("静态更新清单验签失败：{error}"))?;
    let mut catalog: Catalog =
        serde_json::from_slice(bytes).map_err(|error| format!("静态更新清单无效：{error}"))?;
    if catalog.schema != 1 {
        return Err("静态更新清单格式不受支持".into());
    }
    validate_manifest(settings.release_repo.trim(), &catalog.installer)?;
    validate_components(&mut catalog);
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    use std::net::TcpListener;

    const PAYLOAD: &str = include_str!("../tests/fixtures/update-catalog.json");
    const SIGNATURE: &str = include_str!("../tests/fixtures/update-catalog.json.minisig");
    const TEST_KEY: &str = "RWTZaaq8Jlp7qRSoKK4tDYfpAslM5P/LTvHSSTox80Pzx0LDZUisoTgH";

    fn paths() -> AppPaths {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "optinr-catalog-{}-{}-{}",
            std::process::id(),
            now_ms(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        AppPaths {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            root,
        }
    }

    fn serve(payload: String, signature: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/update-catalog.json",
            listener.local_addr().unwrap()
        );
        let worker = std::thread::spawn(move || {
            for body in [payload, signature] {
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
                }
                assert!(!request.contains("api.github.com"));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (url, worker)
    }

    #[test]
    fn accepts_any_stable_tag_and_rejects_prerelease_markers() {
        for (tag, expected) in [
            ("v1.13.3", true),
            ("v1.13.3.3", true),
            ("1.13.3.4", true),
            ("v1.13", true),
            ("2026.10.25", true),
            ("v1.13.0-alpha8", false),
            ("1.13.3-rc.1", false),
            ("v1.13.3-beta", false),
            ("preview-1.13.3", false),
            ("latest", false),
            ("", false),
            ("v1.13.3 (hotfix)", false),
        ] {
            assert_eq!(is_stable_tag(tag), expected, "{tag}");
        }
    }

    #[test]
    fn unstable_upstream_only_disables_upstream_component() {
        let mut catalog: Catalog = serde_json::from_str(PAYLOAD).unwrap();
        validate_components(&mut catalog);
        assert!(catalog.upstream.is_some());
        assert!(catalog.upstream_error.is_none());
        assert!(catalog.reframework.is_some());

        let mut catalog: Catalog = serde_json::from_str(PAYLOAD).unwrap();
        catalog.upstream.as_mut().unwrap().tag = "v1.13.3.4-rc1".into();
        validate_components(&mut catalog);
        assert!(catalog.upstream.is_none());
        assert!(
            catalog
                .upstream_error
                .as_deref()
                .unwrap()
                .contains("预发布")
        );
        assert_eq!(
            crate::self_update::available(&catalog.installer)
                .unwrap()
                .unwrap()
                .version,
            "0.3.0"
        );

        let mut catalog: Catalog = serde_json::from_str(PAYLOAD).unwrap();
        catalog.upstream = None;
        catalog.reframework.as_mut().unwrap().asset.size = 0;
        validate_components(&mut catalog);
        assert!(catalog.upstream.is_none());
        assert!(catalog.reframework.is_none());
        assert!(catalog.reframework_error.is_some());
        assert!(crate::self_update::available(&catalog.installer).is_ok());
    }

    #[test]
    fn verifies_catalog_and_rejects_tampering_wrong_key_and_wrong_source() {
        let settings = Settings::default();
        let catalog = verify(PAYLOAD.as_bytes(), SIGNATURE, &settings, TEST_KEY).unwrap();
        assert_eq!(catalog.upstream.as_ref().unwrap().tag, "v1.13.0");
        assert_eq!(
            catalog.reframework.as_ref().unwrap().asset.sha256,
            "27376e1da177ddcde9c46b3541bdc6fa1ad54a11d0cab9f7782eff0481455e94"
        );
        assert_eq!(
            crate::self_update::available(&catalog.installer)
                .unwrap()
                .unwrap()
                .version,
            "0.3.0"
        );
        assert!(
            verify(
                PAYLOAD.replace("v1.13.0", "v1.14.0").as_bytes(),
                SIGNATURE,
                &settings,
                TEST_KEY
            )
            .is_err()
        );
        assert!(verify(PAYLOAD.as_bytes(), SIGNATURE, &settings, "bad-key").is_err());
        let mut settings = settings;
        settings.release_repo = "someone/else".into();
        assert!(verify(PAYLOAD.as_bytes(), SIGNATURE, &settings, TEST_KEY).is_err());
        for repo in ["", "../other", "a/b/c", "a/b?evil", "a/.."] {
            settings.release_repo = repo.into();
            assert!(catalog_url(&settings).is_err());
        }
    }

    #[test]
    fn caches_for_one_hour_manual_refresh_backs_off_and_uses_only_verified_cache() {
        let paths = paths();
        let settings = Settings::default();
        let (url, worker) = serve(PAYLOAD.into(), SIGNATURE.into());
        let first = check_from(&paths, &settings, &url, TEST_KEY, false).unwrap();
        worker.join().unwrap();
        assert!(first.warning.is_none());
        assert!(
            check_from(&paths, &settings, &url, TEST_KEY, false)
                .unwrap()
                .warning
                .is_none()
        );
        // The test server is now closed: only a forced refresh may attempt another request.
        let offline = check_from(&paths, &settings, &url, TEST_KEY, true).unwrap();
        assert!(offline.warning.is_some());
        assert_eq!(
            offline.upstream.as_ref().unwrap().checked_at_ms,
            first.upstream.as_ref().unwrap().checked_at_ms
        );
        let cache_path = paths.updates.join("catalog-cache.json");
        let cache = read_json::<Cache>(&cache_path).unwrap();
        assert_eq!(cache.payload.as_deref(), Some(PAYLOAD));
        assert!(cache.retry_after_ms > now_ms());
        assert!(
            check_from(&paths, &settings, &url, TEST_KEY, true)
                .unwrap()
                .warning
                .is_some()
        );
        assert_eq!(
            read_json::<Cache>(&cache_path).unwrap().retry_after_ms,
            cache.retry_after_ms
        );

        let mut cache = cache;
        cache.payload = Some(PAYLOAD.replace("v1.13.0", "v9.99.0"));
        write_json_atomic(&cache_path, &cache).unwrap();
        assert!(check_from(&paths, &settings, &url, TEST_KEY, false).is_err());

        cache.payload = Some(PAYLOAD.into());
        cache.error = None;
        cache.retry_after_ms = 0;
        cache.checked_at_ms = now_ms() - CHECK_INTERVAL_MS - 1;
        write_json_atomic(&cache_path, &cache).unwrap();
        assert!(
            check_from(&paths, &settings, &url, TEST_KEY, false)
                .unwrap()
                .warning
                .is_some()
        );
        assert!(cached(&paths, &settings).is_none());
        let (url, worker) = serve(PAYLOAD.into(), SIGNATURE.into());
        cache.url = url.clone();
        cache.checked_at_ms = now_ms();
        cache.retry_after_ms = 0;
        cache.error = Some("previous offline failure".into());
        write_json_atomic(&cache_path, &cache).unwrap();
        assert!(
            check_from(&paths, &settings, &url, TEST_KEY, false)
                .unwrap()
                .warning
                .is_none()
        );
        worker.join().unwrap();
        crate::util::remove_tree_safely(&paths.root).unwrap();
    }

    #[test]
    fn signature_failure_preserves_previous_catalog_and_no_cache_failure_is_throttled() {
        let paths = paths();
        let settings = Settings::default();
        let (url, worker) = serve(PAYLOAD.into(), "invalid-signature".into());
        assert!(check_from(&paths, &settings, &url, TEST_KEY, true).is_err());
        worker.join().unwrap();
        let cache_path = paths.updates.join("catalog-cache.json");
        let mut cache = read_json::<Cache>(&cache_path).unwrap();
        assert!(cache.payload.is_none());
        let retry = cache.retry_after_ms;
        assert!(check_from(&paths, &settings, &url, TEST_KEY, true).is_err());
        assert_eq!(
            read_json::<Cache>(&cache_path).unwrap().retry_after_ms,
            retry
        );

        let (url, worker) = serve(PAYLOAD.replace("v1.13.0", "v9.99.0"), SIGNATURE.into());
        cache.url = url.clone();
        cache.payload = Some(PAYLOAD.into());
        cache.signature = Some(SIGNATURE.into());
        cache.checked_at_ms = 1;
        cache.retry_after_ms = 0;
        write_json_atomic(&cache_path, &cache).unwrap();
        let fallback = check_from(&paths, &settings, &url, TEST_KEY, true).unwrap();
        worker.join().unwrap();
        assert!(fallback.warning.as_deref().unwrap().contains("验签失败"));
        assert_eq!(fallback.upstream.as_ref().unwrap().tag, "v1.13.0");
        assert_eq!(
            read_json::<Cache>(&cache_path).unwrap().payload.as_deref(),
            Some(PAYLOAD)
        );
        crate::util::remove_tree_safely(&paths.root).unwrap();
    }
}
