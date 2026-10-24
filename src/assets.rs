use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::{extract_zip, find_release_root, read_small_text};
use crate::model::{AppPaths, ExtrasManifest, ModelSelection, Progress, Settings, UpstreamRelease};
use crate::updates::{check_upstream, download_verified, load_cached_upstream};
use crate::util::{format_bytes, remove_tree_safely, safe_relative, sha256_file};

#[derive(Debug, Clone)]
pub struct PreparedComponents {
    pub models: ModelSelection,
    pub release_root: PathBuf,
    pub extras_root: PathBuf,
    pub upstream_tag: String,
    pub extras_version: String,
}

pub fn prepare(
    paths: &AppPaths,
    settings: &Settings,
    upstream: Option<UpstreamRelease>,
    include_extras: bool,
    mut progress: impl FnMut(Progress),
) -> Result<PreparedComponents, String> {
    let upstream = if settings.local_upstream_source.is_some() {
        None
    } else {
        Some(
            match upstream.or_else(|| load_cached_upstream(paths, settings)) {
                Some(release) => release,
                None => check_upstream(paths, settings)?,
            },
        )
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
            progress(Progress::new(
                "准备组件",
                format!(
                    "下载 {} · {}",
                    upstream.tag,
                    format_bytes(upstream.asset.size)
                ),
                Some(0.0),
            ));
            let destination = crate::updates::download_upstream(paths, upstream, &mut progress)?;
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
    validate_release(&release_root, settings.models)?;
    let actual_version = read_small_text(&release_root.join("VERSION"), 1024)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let upstream_tag = actual_version.unwrap_or(upstream_tag);

    let (extras_root, extras_version) = if include_extras {
        let manifest = fixed_extras()?;
        let (extras_root, extras_version) =
            if let Some(source) = settings.local_extras_source.as_deref() {
                if !source.is_dir() {
                    return Err("固定资源已改为目录导入，请重新选择目录".to_string());
                }
                progress(Progress::new(
                    "准备组件",
                    format!("使用本地固定资源: {}", source.display()),
                    None,
                ));
                (source.to_path_buf(), "local".to_string())
            } else {
                let destination = paths.cache.join("extras").join(&manifest.version);
                download_extras(&manifest, &destination, settings.models, &mut progress)?;
                (destination, manifest.version.clone())
            };
        validate_extras(&extras_root, &manifest, settings.models)?;
        validate_model_compatibility(&release_root, &extras_root, settings.models)?;
        (extras_root, extras_version)
    } else {
        (PathBuf::new(), String::new())
    };

    Ok(PreparedComponents {
        models: settings.models,
        release_root,
        extras_root,
        upstream_tag,
        extras_version,
    })
}

pub fn fixed_extras() -> Result<ExtrasManifest, String> {
    let manifest: ExtrasManifest = serde_json::from_str(include_str!("../release/extras-v1.json"))
        .map_err(|e| format!("内置固定资源清单无效: {e}"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &ExtrasManifest) -> Result<(), String> {
    let url =
        reqwest::Url::parse(&manifest.base_url).map_err(|e| format!("固定资源地址无效: {e}"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !manifest.base_url.ends_with('/')
        || manifest.version.is_empty()
        || safe_label(&manifest.version) != manifest.version
        || matches!(manifest.version.as_str(), "." | "..")
    {
        return Err("固定资源版本或 HTTPS 地址无效".to_string());
    }
    let mut paths = std::collections::HashSet::new();
    for file in &manifest.files {
        for path in [&file.path, &file.key] {
            safe_relative(Path::new(path))?;
            if !path
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-/".contains(&c))
            {
                return Err(format!("固定资源路径无效: {path}"));
            }
        }
        if !paths.insert(file.path.as_str())
            || file.key
                != format!(
                    "{}/{}",
                    if file.path.ends_with("dlssnr-small.bin") {
                        "extras-v2"
                    } else {
                        "extras-v1"
                    },
                    file.path
                )
            || file.size == 0
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(format!("固定资源清单条目无效: {}", file.path));
        }
    }
    let required = [
        "MFGUnlocker.asi",
        "dlssnr-amd/dlssnr.bin",
        "dlssnr-amd/dlssnr-int4.bin",
        "dlssnr-amd/dlssnr-small.bin",
    ];
    if paths != required.into_iter().collect() {
        return Err("固定资源清单必须包含且只包含四个安装文件".to_string());
    }
    Ok(())
}

fn download_extras(
    manifest: &ExtrasManifest,
    destination: &Path,
    models: ModelSelection,
    progress: &mut impl FnMut(Progress),
) -> Result<(), String> {
    let files: Vec<_> = manifest
        .files
        .iter()
        .filter(|file| models.includes(&file.path))
        .collect();
    for (index, file) in files.iter().enumerate() {
        download_verified(
            &format!("{}{}", manifest.base_url, file.key),
            &destination.join(safe_relative(Path::new(&file.path))?),
            &file.sha256,
            Some(file.size),
            "下载固定资源",
            |update| {
                progress(Progress::new(
                    update.stage,
                    format!(
                        "{}/{} · {} · {}",
                        index + 1,
                        files.len(),
                        file.path,
                        update.detail
                    ),
                    update
                        .fraction
                        .map(|fraction| (index as f32 + fraction) / files.len() as f32),
                ));
            },
        )?;
    }
    Ok(())
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
    if destination.exists() {
        remove_tree_safely(&destination)?;
    }
    fs::create_dir_all(&destination).map_err(|e| format!("创建 staging 失败: {e}"))?;
    extract_zip(source, &destination, progress_name, &mut *progress)?;
    crate::util::write_atomic(&destination.join(".__complete"), b"ok")?;
    Ok(destination)
}

fn validate_release(root: &Path, models: ModelSelection) -> Result<(), String> {
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
    if models != ModelSelection::Original {
        let descriptor = root.join("model-tools/descriptor-small.json");
        if !descriptor.is_file() {
            return Err("所选主包不支持 310.9.16 新模型，请更新至 v1.13.3 或更新版本".to_string());
        }
    }
    Ok(())
}

fn validate_extras(
    root: &Path,
    manifest: &ExtrasManifest,
    models: ModelSelection,
) -> Result<(), String> {
    for file in manifest
        .files
        .iter()
        .filter(|file| models.includes(&file.path))
    {
        let path = root.join(safe_relative(Path::new(&file.path))?);
        if !path.is_file() {
            return Err(format!("固定资源缺少 {}: {}", file.path, root.display()));
        }
        if path.metadata().map_err(|e| e.to_string())?.len() != file.size
            || !sha256_file(&path)?.eq_ignore_ascii_case(&file.sha256)
        {
            return Err(format!("固定资源大小或 SHA-256 不匹配: {}", file.path));
        }
    }
    Ok(())
}

fn validate_model_compatibility(
    release_root: &Path,
    extras_root: &Path,
    models: ModelSelection,
) -> Result<(), String> {
    if models == ModelSelection::Small {
        return Ok(());
    }
    let checksum_path = release_root.join("model-tools").join("dlssnr-int4.sha256");
    let checksum = read_small_text(&checksum_path, 4096)?;
    let expected = checksum
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{} 不是有效的 SHA-256 清单", checksum_path.display()))?;
    let int4 = extras_root.join("dlssnr-amd").join("dlssnr-int4.bin");
    let actual = sha256_file(&int4)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "固定资源中的 dlssnr-int4.bin 与新主包不兼容：实际 {actual}，主包期望 {expected}。请等待维护者发布新版 extra。"
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "downloads real Cloudflare files; requires NR_TEST_UPSTREAM_ZIP"]
    fn cloudflare_live_install_and_uninstall() {
        let upstream = PathBuf::from(
            std::env::var_os("NR_TEST_UPSTREAM_ZIP").expect("set NR_TEST_UPSTREAM_ZIP"),
        );
        assert_eq!(
            sha256_file(&upstream).unwrap(),
            "779b9b5f4d5756f663798e796c2974d09f05077bde9df1ff5ace31e087ce4dfe"
        );
        let root = std::env::temp_dir().join(format!(
            "optinr-cdn-live-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        let paths = AppPaths {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: root.join("backups"),
            games: root.join("records"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            root: root.clone(),
        };
        let game_dir = root.join("game");
        fs::create_dir_all(&game_dir).unwrap();
        let exe = game_dir.join("CloudflareMigrationTest.exe");
        // An inert PE header for the parser; this fixture is never executed.
        let mut pe = vec![0u8; 512];
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x94..0x96].copy_from_slice(&0xf0u16.to_le_bytes());
        pe[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        fs::write(&exe, &pe).unwrap();
        let game = crate::games::inspect_manual(exe.clone()).unwrap();
        let mut settings: Settings = serde_json::from_str(
            r#"{"extras":{"version":"obsolete","url":"http://127.0.0.1:9/obsolete.zip"}}"#,
        )
        .unwrap();
        settings.local_upstream_source = Some(upstream);
        let partial_dir = paths.cache.join("extras/extras-v2");
        fs::create_dir_all(&partial_dir).unwrap();
        let response = crate::updates::http_client()
            .unwrap()
            .get(format!(
                "{}{}",
                fixed_extras().unwrap().base_url,
                "extras-v1/MFGUnlocker.asi"
            ))
            .header("Range", "bytes=0-1048575")
            .send()
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        let prefix = response.bytes().unwrap();
        assert_eq!(prefix.len(), 1048576);
        fs::write(partial_dir.join("MFGUnlocker.part"), prefix).unwrap();
        let started = Instant::now();
        let mut stages = Vec::new();
        let components = prepare(&paths, &settings, None, true, |update| {
            if stages.last() != Some(&update.stage) {
                println!("{}: {}", update.stage, update.detail);
                stages.push(update.stage);
            }
        })
        .unwrap();
        assert!(!stages.iter().any(|stage| stage == "解压固定资源"));
        assert_eq!(components.extras_version, "extras-v2");
        assert_eq!(fs::read_dir(&components.extras_root).unwrap().count(), 2);
        let report = crate::install::install(&paths, &game, &components, |_| {}).unwrap();
        println!(
            "{}; Cloudflare preparation {:.1}s",
            report.title,
            started.elapsed().as_secs_f64()
        );
        let check = crate::install::check_install(&paths, &game).unwrap();
        assert_eq!(check.title, "检查通过");
        assert!(!game_dir.join("nvngx_dlssnr.dll").exists());
        assert!(!game_dir.join("dlssnr_on_amd_weights.bin").exists());
        for file in fixed_extras()
            .unwrap()
            .files
            .iter()
            .filter(|file| settings.models.includes(&file.path))
        {
            let deployed = if file.path == "MFGUnlocker.asi" {
                "OptiScaler/plugins/MFGUnlocker.asi"
            } else {
                &file.path
            };
            assert_eq!(sha256_file(&game_dir.join(deployed)).unwrap(), file.sha256);
        }
        crate::install::uninstall(&paths, &game).unwrap();
        assert_eq!(fs::read_dir(&game_dir).unwrap().count(), 1);
        assert_eq!(fs::read(&exe).unwrap(), pe);
        crate::util::remove_tree_safely(&root).unwrap();
        println!("PASS: real CDN resume, installation, managed-file check, and uninstall");
    }

    #[test]
    fn selected_models_require_compatible_runtime_and_int4() {
        let root = std::env::temp_dir().join(format!(
            "optinr-compat-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(root.join("model-tools")).unwrap();
        for dir in ["OptiScaler", "dlssnr-amd", "person-model"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in ["OptiScaler.dll", "OptiScaler.ini", "MochizukiNrRuntime.dll"] {
            fs::write(root.join(file), "runtime").unwrap();
        }
        assert!(validate_release(&root, ModelSelection::Small).is_err());
        fs::write(root.join("model-tools/descriptor-small.json"), "{}").unwrap();
        validate_model_compatibility(&root, &root, ModelSelection::Small).unwrap();
        assert!(validate_model_compatibility(&root, &root, ModelSelection::Both).is_err());
        validate_release(&root, ModelSelection::Both).unwrap();
        let int4 = root.join("dlssnr-amd/dlssnr-int4.bin");
        fs::write(&int4, "weights").unwrap();
        fs::write(
            root.join("model-tools/dlssnr-int4.sha256"),
            sha256_file(&int4).unwrap(),
        )
        .unwrap();
        validate_model_compatibility(&root, &root, ModelSelection::Both).unwrap();
        fs::write(&int4, "corrupt").unwrap();
        assert!(validate_model_compatibility(&root, &root, ModelSelection::Both).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn embedded_manifest_and_legacy_settings() {
        let manifest = fixed_extras().unwrap();
        assert_eq!(manifest.files.len(), 4);
        assert!(
            manifest
                .base_url
                .starts_with("https://downloads.bonvoyage.dpdns.org/")
        );
        for path in [
            "../escape.bin",
            "/absolute.bin",
            "C:/absolute.bin",
            "a%2fb.bin",
            "a\\b.bin",
        ] {
            let mut bad = manifest.clone();
            bad.files[0].path = path.into();
            bad.files[0].key = format!("{}/{path}", bad.version);
            assert!(validate_manifest(&bad).is_err(), "accepted {path}");
        }
        let mut bad = manifest.clone();
        bad.files.pop();
        assert!(validate_manifest(&bad).is_err());
        let settings: Settings = serde_json::from_str(
            r#"{"release_repo":"example/repo","extras":{"url":"https://old.example/extras.zip"},"local_extras_source":"local-resources"}"#,
        ).unwrap();
        assert_eq!(settings.release_repo, "example/repo");
        assert_eq!(
            settings.local_extras_source,
            Some(PathBuf::from("local-resources"))
        );
        assert!(
            serde_json::to_value(settings)
                .unwrap()
                .get("extras")
                .is_none()
        );
    }

    #[test]
    fn direct_files_resume_reuse_and_stop_on_corruption() {
        use sha2::Digest;
        for (models, corrupt) in [
            (ModelSelection::Original, false),
            (ModelSelection::Small, false),
            (ModelSelection::Both, false),
            (ModelSelection::Original, true),
        ] {
            let root = std::env::temp_dir().join(format!(
                "optinr-direct-{}-{}-{models:?}-{corrupt}",
                std::process::id(),
                crate::util::now_ms()
            ));
            fs::create_dir_all(&root).unwrap();
            let mut manifest = fixed_extras().unwrap();
            manifest.files.retain(|file| models.includes(&file.path));
            let bodies: Vec<Vec<u8>> = (0..manifest.files.len())
                .map(|i| format!("resource-{i}").into_bytes())
                .collect();
            for (file, body) in manifest.files.iter_mut().zip(&bodies) {
                file.size = body.len() as u64;
                file.sha256 = crate::util::hex_lower(&sha2::Sha256::digest(body));
            }
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            manifest.base_url = format!("http://{}/", listener.local_addr().unwrap());
            let remote_files = manifest.files.clone();
            let worker = std::thread::spawn(move || {
                let mut requests = Vec::new();
                let expected_requests = if corrupt { 4 } else { remote_files.len() };
                let deadline = Instant::now() + Duration::from_secs(15);
                while requests.len() < expected_requests {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing HTTP requests");
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = String::new();
                    let mut reader = BufReader::new(&mut stream);
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        request.push_str(&line);
                    }
                    let index = remote_files
                        .iter()
                        .position(|file| request.starts_with(&format!("GET /{} ", file.key)))
                        .unwrap();
                    let body = if corrupt && index == 1 {
                        vec![b'x'; bodies[index].len()]
                    } else {
                        bodies[index].clone()
                    };
                    let offset = if index == 0 {
                        assert!(request.to_ascii_lowercase().contains("range: bytes=3-"));
                        3
                    } else {
                        0
                    };
                    let range = if offset > 0 {
                        format!(
                            "Content-Range: bytes {offset}-{}/{}\r\n",
                            body.len() - 1,
                            body.len()
                        )
                    } else {
                        String::new()
                    };
                    let status = if offset > 0 {
                        "206 Partial Content"
                    } else {
                        "200 OK"
                    };
                    write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{range}Connection: close\r\n\r\n", body.len() - offset).unwrap();
                    stream.write_all(&body[offset..]).unwrap();
                    requests.push(request);
                }
                requests
            });
            fs::write(
                root.join(&manifest.files[0].path).with_extension("part"),
                b"res",
            )
            .unwrap();
            assert!(validate_extras(&root, &manifest, models).is_err());
            let mut fractions = Vec::new();
            let result = download_extras(&manifest, &root, models, &mut |update| {
                assert_eq!(update.stage, "下载固定资源");
                if let Some(fraction) = update.fraction {
                    fractions.push(fraction);
                }
            });
            let requests = worker.join().unwrap();
            if corrupt {
                assert!(result.unwrap_err().contains("SHA-256"));
                assert!(!root.join(&manifest.files[2].path).exists());
                assert!(validate_extras(&root, &manifest, models).is_err());
                assert_eq!(requests.len(), 4);
            } else {
                result.unwrap();
                validate_extras(&root, &manifest, models).unwrap();
                assert_eq!(fractions.last(), Some(&1.0));
                assert!(fractions.windows(2).all(|pair| pair[0] <= pair[1]));
                // The server has exited: a second download must use verified files.
                download_extras(&manifest, &root, models, &mut |_| {}).unwrap();
                fs::write(root.join(&manifest.files[0].path), b"xxxxxxxxxx").unwrap();
                assert!(validate_extras(&root, &manifest, models).is_err());
                assert_eq!(requests.len(), manifest.files.len());
            }
            crate::util::remove_tree_safely(&root).unwrap();
        }
    }
}
