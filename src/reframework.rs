use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{AppPaths, GameInfo, MaintenanceReport, Progress, Settings};
use crate::util::{game_id, now_ms, process_is_running, read_json, sha256_file, write_json_atomic};

#[derive(Debug, Serialize, Deserialize)]
struct ManagedReframework {
    schema: u32,
    tag: String,
    sha256: String,
    installed_at_ms: u64,
}

pub fn install(
    paths: &AppPaths,
    settings: &Settings,
    game: &GameInfo,
    mut progress: impl FnMut(Progress),
) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再安装 REFramework".to_string());
    }
    progress(Progress::new(
        "REFramework",
        "读取已验签更新清单中的 REFramework 版本",
        Some(0.05),
    ));
    let release = crate::catalog::check(paths, settings, true)?
        .reframework
        .ok_or_else(|| "更新清单尚未包含 REFramework nightly".to_string())?;
    let asset = &release.asset;
    let archive = paths.staging.join(format!("reframework-{}.zip", asset.id));
    crate::updates::download_verified(
        &asset.url,
        &archive,
        &asset.sha256,
        Some(asset.size),
        "下载 REFramework",
        &mut progress,
    )?;
    let extracted = paths.staging.join(format!("reframework-{}", asset.id));
    if extracted.exists() {
        crate::util::remove_tree_safely(&extracted)?;
    }
    crate::archive::extract_zip(&archive, &extracted, "解压 REFramework", &mut progress)?;
    let source = find_dinput8(&extracted)?;
    let dll_sha256 = sha256_file(&source)?;
    let target = game.game_dir.join("dinput8.dll");
    let replaced = target.is_file();
    crate::install::copy_replacing(&source, &target)?;
    let state_dir = paths.games.join(game_id(&game.exe));
    fs::create_dir_all(&state_dir)
        .map_err(|error| format!("创建 REFramework 记录目录失败: {error}"))?;
    write_json_atomic(
        &state_dir.join("reframework.json"),
        &ManagedReframework {
            schema: 1,
            tag: release.tag.clone(),
            sha256: dll_sha256.clone(),
            installed_at_ms: now_ms(),
        },
    )?;
    let mut report = MaintenanceReport {
        title: "REFramework 安装完成".to_string(),
        lines: vec![
            format!("版本：{}", release.tag),
            format!("文件：{}", target.display()),
            format!("SHA-256：{dll_sha256}"),
            if replaced {
                "已覆盖游戏目录中原有的 dinput8.dll".to_string()
            } else {
                "已安装 dinput8.dll".to_string()
            },
        ],
    };
    if let Err(error) = crate::install::cleanup_installer_files(paths) {
        report
            .lines
            .push(format!("安装已完成，但组件缓存清理失败：{error}"));
    }
    Ok(report)
}

pub fn remove_managed(paths: &AppPaths, game: &GameInfo) -> Result<Option<String>, String> {
    let record_path = paths
        .games
        .join(game_id(&game.exe))
        .join("reframework.json");
    if !record_path.is_file() {
        return Ok(None);
    }
    let record = read_json::<ManagedReframework>(&record_path)
        .map_err(|error| format!("REFramework 安装记录损坏: {error}"))?;
    let target = game.game_dir.join("dinput8.dll");
    let mut message = "REFramework 安装记录已移除".to_string();
    if target.is_file() {
        if sha256_file(&target)?.eq_ignore_ascii_case(&record.sha256) {
            fs::remove_file(&target)
                .map_err(|error| format!("删除 {} 失败: {error}", target.display()))?;
            message = "已移除 REFramework dinput8.dll".to_string();
        } else {
            message = "dinput8.dll 已被修改，保留原文件".to_string();
        }
    }
    fs::remove_file(&record_path).map_err(|error| format!("删除 REFramework 记录失败: {error}"))?;
    Ok(Some(message))
}

fn find_dinput8(root: &Path) -> Result<PathBuf, String> {
    let direct = root.join("dinput8.dll");
    if direct.is_file() {
        return Ok(direct);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|error| format!("读取 {} 失败: {error}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("dinput8.dll"))
            {
                return Ok(path);
            }
        }
    }
    Err("REFramework.zip 中找不到 dinput8.dll".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dinput8_recursively() {
        let root = std::env::temp_dir().join(format!(
            "reframework-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested").join("dinput8.dll"), "dll").unwrap();
        assert_eq!(
            find_dinput8(&root).unwrap(),
            root.join("nested").join("dinput8.dll")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uninstall_removes_only_unmodified_managed_dll() {
        let root = std::env::temp_dir().join(format!(
            "reframework-uninstall-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let game_dir = root.join("game");
        fs::create_dir_all(&game_dir).unwrap();
        let exe = game_dir.join("PRAGMATA.exe");
        fs::write(&exe, "game").unwrap();
        let target = game_dir.join("dinput8.dll");
        fs::write(&target, "managed").unwrap();
        let paths = AppPaths {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            root: root.clone(),
        };
        let game = GameInfo {
            launcher: "Steam".to_string(),
            name: "PRAGMATA".to_string(),
            exe: exe.clone(),
            game_dir: game_dir.clone(),
            architecture: Some(64),
            imports: vec![],
            compatibility: crate::model::Compatibility::Manual,
        };
        let state_dir = paths.games.join(game_id(&exe));
        fs::create_dir_all(&state_dir).unwrap();
        write_json_atomic(
            &state_dir.join("reframework.json"),
            &ManagedReframework {
                schema: 1,
                tag: "nightly-test".to_string(),
                sha256: sha256_file(&target).unwrap(),
                installed_at_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(
            remove_managed(&paths, &game).unwrap().as_deref(),
            Some("已移除 REFramework dinput8.dll")
        );
        assert!(!target.exists());
        assert!(!state_dir.join("reframework.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
