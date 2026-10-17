use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::archive::copy_file;
use crate::assets::PreparedComponents;
use crate::ini;
use crate::model::{AppPaths, GameInfo, InstallRecord, MaintenanceReport, ManagedFile, Progress};
use crate::util::{
    file_contains_marker, game_id, now_ms, process_is_running, read_json, safe_relative,
    sha256_file, write_json_atomic,
};

#[derive(Debug, Clone)]
struct PlannedFile {
    source: PathBuf,
    relative: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransactionEntry {
    target: PathBuf,
    backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Transaction {
    schema: u32,
    game_dir: PathBuf,
    backup_root: PathBuf,
    entries: Vec<TransactionEntry>,
    started_at_ms: u64,
}

pub fn install(
    paths: &AppPaths,
    game: &GameInfo,
    components: &PreparedComponents,
    mut progress: impl FnMut(Progress),
) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再安装".to_string());
    }
    let id = game_id(&game.exe);
    let state_dir = paths.games.join(&id);
    fs::create_dir_all(&state_dir).map_err(|e| format!("创建安装记录目录失败: {e}"))?;
    let record_path = state_dir.join("record.json");
    let previous_record_path = state_dir.join("previous-record.json");
    let existing_record = read_json::<InstallRecord>(&record_path).ok();
    let proxy = choose_proxy(&game.game_dir, existing_record.as_ref())?;
    let ini_path = game.game_dir.join("OptiScaler.ini");
    let template_ini = components.release_root.join("OptiScaler.ini");

    progress(Progress::new("预检查", "生成固定方案文件计划", Some(0.02)));
    let mut plan = build_plan(components, &proxy)?;
    if ini_path.is_file() {
        plan.retain(|file| file.relative != "OptiScaler.ini");
    } else if !template_ini.is_file() {
        return Err("主组件缺少 OptiScaler.ini".to_string());
    }
    let duplicate = plan
        .iter()
        .map(|file| file.relative.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    if duplicate.len() != plan.len() {
        return Err("文件计划存在大小写冲突".to_string());
    }

    let stamp = now_ms().to_string();
    let backup_root = paths.backups.join(&id).join(&stamp);
    fs::create_dir_all(&backup_root).map_err(|e| format!("创建备份目录失败: {e}"))?;
    progress(Progress::new(
        "备份",
        "备份游戏目录中的现有文件",
        Some(0.08),
    ));
    let mut entries = Vec::new();
    let mut backed_up = HashSet::new();
    for file in &plan {
        let target = game.game_dir.join(&file.relative);
        add_transaction_entry(
            &game.game_dir,
            &backup_root,
            &target,
            &mut entries,
            &mut backed_up,
        )?;
    }
    add_transaction_entry(
        &game.game_dir,
        &backup_root,
        &ini_path,
        &mut entries,
        &mut backed_up,
    )?;
    let transaction_path = state_dir.join("transaction.json");
    let transaction = Transaction {
        schema: 1,
        game_dir: game.game_dir.clone(),
        backup_root: backup_root.clone(),
        entries: entries.clone(),
        started_at_ms: now_ms(),
    };
    write_json_atomic(&transaction_path, &transaction)?;

    let result = (|| {
        if !ini_path.is_file() {
            progress(Progress::new("应用", "创建 OptiScaler.ini", None));
            copy_replacing(&template_ini, &ini_path)?;
        }
        let total = plan.len().max(1);
        for (index, file) in plan.iter().enumerate() {
            let target = game.game_dir.join(&file.relative);
            let needs_copy = match sha256_file(&target) {
                Ok(hash) => !hash.eq_ignore_ascii_case(&file.sha256),
                Err(_) => true,
            };
            if needs_copy {
                progress(Progress::new(
                    "应用",
                    format!("{}/{}  {}", index + 1, total, file.relative),
                    Some(0.12 + 0.72 * index as f32 / total as f32),
                ));
                copy_replacing(&file.source, &target)?;
            }
        }
        progress(Progress::new(
            "写入配置",
            "合并 Mochizuki NR 与 XeFG 固定配置",
            Some(0.88),
        ));
        ini::apply_fixed_profile(&ini_path, false)?;

        progress(Progress::new("验证", "校验所有受管文件", Some(0.94)));
        let mut managed = Vec::new();
        for file in &plan {
            if file.relative.eq_ignore_ascii_case("OptiScaler.ini") {
                continue;
            }
            let target = game.game_dir.join(&file.relative);
            let actual = sha256_file(&target)?;
            if !actual.eq_ignore_ascii_case(&file.sha256) {
                return Err(format!("安装后哈希不符: {}", file.relative));
            }
            managed.push(ManagedFile {
                relative_path: file.relative.clone(),
                sha256: actual,
            });
        }
        managed.push(ManagedFile {
            relative_path: "OptiScaler.ini".to_string(),
            sha256: sha256_file(&ini_path)?,
        });
        managed.sort_by(|a, b| {
            a.relative_path
                .to_ascii_lowercase()
                .cmp(&b.relative_path.to_ascii_lowercase())
        });

        if let Some(previous) = existing_record.as_ref() {
            copy_file(&record_path, &previous_record_path)?;
            progress(Progress::new(
                "提交",
                format!("从 {} 升级", previous.upstream_tag),
                Some(0.98),
            ));
        }
        let record = InstallRecord {
            schema: 1,
            game_name: game.name.clone(),
            exe: game.exe.clone(),
            installed_at_ms: now_ms(),
            upstream_tag: components.upstream_tag.clone(),
            extras_version: Some(components.extras_version.clone()),
            profile: crate::model::FIXED_PROFILE.to_string(),
            proxy_file: proxy.clone(),
            frame_gen_enabled: true,
            files: managed,
            baseline_backup: existing_record
                .as_ref()
                .map(|record| record.baseline_backup.clone())
                .unwrap_or_else(|| backup_root.clone()),
            previous_snapshot: backup_root.clone(),
        };
        write_json_atomic(&record_path, &record)?;
        fs::remove_file(&transaction_path).map_err(|e| format!("提交事务记录失败: {e}"))?;
        Ok(MaintenanceReport {
            title: "安装完成".to_string(),
            lines: vec![
                format!("游戏：{}", game.name),
                format!("组件：{}", components.upstream_tag),
                format!("代理：{proxy}"),
                format!("固定资源：{}", components.extras_version),
                format!("受管文件：{} 个", record.files.len()),
            ],
        })
    })();

    if let Err(error) = &result {
        return match rollback_transaction(&transaction) {
            Ok(()) => {
                let _ = fs::remove_file(&transaction_path);
                result
            }
            Err(rollback_error) => Err(format!(
                "{error}\n自动回滚未完成：{rollback_error}\n事务日志已保留，下次启动会继续恢复。"
            )),
        };
    }
    result
}

pub fn check_install(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    let record = load_record(paths, game)?;
    let mut issues = Vec::new();
    for file in &record.files {
        let target = record_target(&game.game_dir, &file.relative_path)?;
        if !target.is_file() {
            issues.push(format!("缺失：{}", file.relative_path));
            continue;
        }
        if file.relative_path.eq_ignore_ascii_case("OptiScaler.ini") {
            issues.extend(
                ini::check_fixed_profile(&target, record.frame_gen_enabled)?
                    .into_iter()
                    .map(|issue| format!("OptiScaler.ini：{issue}")),
            );
            continue;
        }
        let actual = sha256_file(&target)?;
        if !actual.eq_ignore_ascii_case(&file.sha256) {
            issues.push(format!("已修改或损坏：{}", file.relative_path));
        }
    }
    let title = if issues.is_empty() {
        "检查通过".to_string()
    } else {
        format!("发现 {} 个问题", issues.len())
    };
    let mut lines = vec![
        format!("已安装组件：{}", record.upstream_tag),
        format!(
            "固定资源：{}",
            record.extras_version.as_deref().unwrap_or("未知")
        ),
        format!("代理：{}", record.proxy_file),
    ];
    lines.extend(issues);
    Ok(MaintenanceReport { title, lines })
}

pub fn uninstall(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再卸载".to_string());
    }
    let record = load_record(paths, game)?;
    let mut lines = Vec::new();
    let mut restored = 0usize;
    let mut removed = 0usize;
    let mut kept = 0usize;
    for file in &record.files {
        let target = record_target(&game.game_dir, &file.relative_path)?;
        if !target.exists() {
            continue;
        }
        let actual = sha256_file(&target)?;
        if !actual.eq_ignore_ascii_case(&file.sha256) {
            kept += 1;
            if lines.len() < 20 {
                lines.push(format!("保留用户改动：{}", file.relative_path));
            }
            continue;
        }
        let baseline = record.baseline_backup.join(&file.relative_path);
        if baseline.is_file() {
            copy_replacing(&baseline, &target)?;
            restored += 1;
        } else {
            fs::remove_file(&target).map_err(|e| format!("删除 {} 失败: {e}", target.display()))?;
            removed += 1;
        }
    }
    let state_dir = paths.games.join(game_id(&game.exe));
    let _ = fs::remove_file(state_dir.join("record.json"));
    let _ = fs::remove_file(state_dir.join("previous-record.json"));
    lines.insert(0, format!("删除 {removed} 个、恢复 {restored} 个受管文件"));
    if kept > 0 {
        lines.insert(1, format!("{kept} 个文件被用户修改，已保留"));
    }
    Ok(MaintenanceReport {
        title: if kept == 0 {
            "卸载完成".to_string()
        } else {
            "卸载完成，部分文件已保留".to_string()
        },
        lines,
    })
}

pub fn disable_frame_gen(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再修改".to_string());
    }
    let mut record = load_record(paths, game)?;
    let ini_path = game.game_dir.join("OptiScaler.ini");
    if !ini_path.is_file() {
        return Err("游戏目录中没有 OptiScaler.ini".to_string());
    }
    let plugin = game
        .game_dir
        .join("OptiScaler")
        .join("plugins")
        .join("MFGUnlocker.asi");
    let mut lines = Vec::new();
    if plugin.is_file() {
        fs::remove_file(&plugin).map_err(|e| format!("移出 MFGUnlocker.asi 失败: {e}"))?;
        record.files.retain(|file| {
            !file
                .relative_path
                .eq_ignore_ascii_case("OptiScaler/plugins/MFGUnlocker.asi")
        });
        lines.push("已移出 MFGUnlocker.asi".to_string());
    }
    ini::apply_fixed_profile(&ini_path, true)?;
    if let Some(ini_entry) = record
        .files
        .iter_mut()
        .find(|file| file.relative_path.eq_ignore_ascii_case("OptiScaler.ini"))
    {
        ini_entry.sha256 = sha256_file(&ini_path)?;
    }
    record.frame_gen_enabled = false;
    write_json_atomic(
        &paths.games.join(game_id(&game.exe)).join("record.json"),
        &record,
    )?;
    lines.push("已关闭 [FrameGen] Enabled".to_string());
    Ok(MaintenanceReport {
        title: "插帧已关闭".to_string(),
        lines,
    })
}

pub fn rollback(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再回滚".to_string());
    }
    let state_dir = paths.games.join(game_id(&game.exe));
    let current = load_record(paths, game)?;
    let previous = read_json::<InstallRecord>(&state_dir.join("previous-record.json"))
        .map_err(|_| "没有上一版安装记录，无法回滚".to_string())?;
    if !current.previous_snapshot.is_dir() {
        return Err("上一版恢复快照不存在".to_string());
    }
    let previous_files = previous
        .files
        .iter()
        .map(|file| file.relative_path.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for file in &current.files {
        if previous_files.contains(&file.relative_path.to_ascii_lowercase()) {
            continue;
        }
        let target = record_target(&game.game_dir, &file.relative_path)?;
        if target.is_file() {
            fs::remove_file(&target)
                .map_err(|e| format!("删除新版文件 {} 失败: {e}", target.display()))?;
        }
    }
    let mut stack = vec![current.previous_snapshot.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|e| format!("读取恢复快照失败: {e}"))?
            .flatten()
        {
            let source = entry.path();
            if entry
                .file_type()
                .map_err(|e| format!("读取快照文件类型失败: {e}"))?
                .is_dir()
            {
                stack.push(source);
            } else {
                let relative = source
                    .strip_prefix(&current.previous_snapshot)
                    .map_err(|_| "恢复快照路径越界".to_string())?;
                copy_replacing(
                    &source,
                    &record_target(&game.game_dir, &relative.to_string_lossy())?,
                )?;
            }
        }
    }
    write_json_atomic(&state_dir.join("record.json"), &previous)?;
    let _ = fs::remove_file(state_dir.join("previous-record.json"));
    Ok(MaintenanceReport {
        title: "回滚完成".to_string(),
        lines: vec![
            format!("恢复到组件版本：{}", previous.upstream_tag),
            format!("恢复快照：{}", current.previous_snapshot.display()),
        ],
    })
}

pub fn recover_transactions(paths: &AppPaths) -> Vec<String> {
    let mut messages = Vec::new();
    let Ok(entries) = fs::read_dir(&paths.games) else {
        return messages;
    };
    for entry in entries.flatten() {
        let state_dir = entry.path();
        let path = state_dir.join("transaction.json");
        let Ok(transaction) = read_json::<Transaction>(&path) else {
            continue;
        };
        if read_json::<InstallRecord>(&state_dir.join("record.json"))
            .is_ok_and(|record| record.installed_at_ms >= transaction.started_at_ms)
        {
            let _ = fs::remove_file(&path);
            messages.push(format!(
                "上次事务已完成提交，清理日志：{}",
                transaction.game_dir.display()
            ));
            continue;
        }
        match rollback_transaction(&transaction) {
            Ok(()) => {
                let _ = fs::remove_file(&path);
                messages.push(format!(
                    "已恢复上次未完成事务：{}",
                    transaction.game_dir.display()
                ));
            }
            Err(error) => messages.push(format!(
                "未完成事务恢复失败 {}: {error}",
                transaction.game_dir.display()
            )),
        }
    }
    messages
}

fn load_record(paths: &AppPaths, game: &GameInfo) -> Result<InstallRecord, String> {
    read_json(&paths.games.join(game_id(&game.exe)).join("record.json"))
        .map_err(|_| "这个游戏还没有本程序的安装记录".to_string())
}

fn build_plan(components: &PreparedComponents, proxy: &str) -> Result<Vec<PlannedFile>, String> {
    let mut files = Vec::new();
    add(
        &mut files,
        &components.release_root.join("OptiScaler.dll"),
        proxy,
    )?;
    add(
        &mut files,
        &components.release_root.join("OptiScaler.ini"),
        "OptiScaler.ini",
    )?;
    add_tree(
        &mut files,
        &components.release_root.join("OptiScaler"),
        "OptiScaler",
    )?;
    add(
        &mut files,
        &components.release_root.join("MochizukiNrRuntime.dll"),
        "MochizukiNrRuntime.dll",
    )?;
    add_tree(
        &mut files,
        &components.release_root.join("dlssnr-amd"),
        "dlssnr-amd",
    )?;
    add_tree(
        &mut files,
        &components.release_root.join("person-model"),
        "person-model",
    )?;
    add(
        &mut files,
        &components.extras_root.join("MFGUnlocker.asi"),
        "OptiScaler/plugins/MFGUnlocker.asi",
    )?;
    add(
        &mut files,
        &components.extras_root.join("dlssnr-amd/dlssnr.bin"),
        "dlssnr-amd/dlssnr.bin",
    )?;
    add(
        &mut files,
        &components.extras_root.join("dlssnr-amd/dlssnr-int4.bin"),
        "dlssnr-amd/dlssnr-int4.bin",
    )?;
    let mut by_path = BTreeMap::new();
    for file in files {
        by_path.insert(file.relative.to_ascii_lowercase(), file);
    }
    Ok(by_path.into_values().collect())
}

fn add(files: &mut Vec<PlannedFile>, source: &Path, relative: &str) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("缺少组件文件: {}", source.display()));
    }
    files.push(PlannedFile {
        source: source.to_path_buf(),
        relative: relative.replace('\\', "/"),
        sha256: sha256_file(source)?,
    });
    Ok(())
}

fn add_tree(files: &mut Vec<PlannedFile>, root: &Path, relative_root: &str) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("缺少组件目录: {}", root.display()));
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|e| format!("读取 {} 失败: {e}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| format!("读取 {} 类型失败: {e}", path.display()))?;
            if file_type.is_symlink() {
                return Err(format!("组件目录包含重解析点: {}", path.display()));
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let suffix = path
                    .strip_prefix(root)
                    .map_err(|_| "组件路径越界".to_string())?;
                let relative = Path::new(relative_root).join(suffix);
                let relative = relative.to_string_lossy().replace('\\', "/");
                if relative
                    .to_ascii_lowercase()
                    .ends_with("plugins/optipatcher.asi")
                {
                    continue;
                }
                add(files, &path, &relative)?;
            }
        }
    }
    Ok(())
}

fn choose_proxy(game_dir: &Path, record: Option<&InstallRecord>) -> Result<String, String> {
    if let Some(record) = record
        && game_dir.join(&record.proxy_file).is_file()
    {
        return Ok(record.proxy_file.clone());
    }
    for candidate in [
        "dxgi.dll",
        "winmm.dll",
        "d3d12.dll",
        "version.dll",
        "dbghelp.dll",
    ] {
        let path = game_dir.join(candidate);
        if !path.exists() || (candidate == "dxgi.dll" && file_contains_marker(&path, "OptiScaler")?)
        {
            return Ok(candidate.to_string());
        }
    }
    Err("常用代理名称均已被游戏目录占用，拒绝覆盖未知 DLL".to_string())
}

fn add_transaction_entry(
    game_dir: &Path,
    backup_root: &Path,
    target: &Path,
    entries: &mut Vec<TransactionEntry>,
    backed_up: &mut HashSet<String>,
) -> Result<(), String> {
    if target.exists() && !target.is_file() {
        return Err(format!("目标不是普通文件: {}", target.display()));
    }
    let relative = target
        .strip_prefix(game_dir)
        .map_err(|_| format!("目标不在游戏目录内: {}", target.display()))?;
    let key = relative.to_string_lossy().to_ascii_lowercase();
    if !backed_up.insert(key) {
        return Ok(());
    }
    let backup = if target.is_file() {
        let destination = backup_root.join(relative);
        copy_file(target, &destination)?;
        Some(destination)
    } else {
        None
    };
    entries.push(TransactionEntry {
        target: target.to_path_buf(),
        backup,
    });
    Ok(())
}

fn rollback_transaction(transaction: &Transaction) -> Result<(), String> {
    let mut errors = Vec::new();
    for entry in transaction.entries.iter().rev() {
        if !entry.target.starts_with(&transaction.game_dir) {
            errors.push(format!(
                "拒绝恢复游戏目录外路径: {}",
                entry.target.display()
            ));
            continue;
        }
        let result = if let Some(backup) = &entry.backup {
            if !backup.starts_with(&transaction.backup_root) {
                Err(format!("拒绝读取备份目录外文件: {}", backup.display()))
            } else {
                copy_replacing(backup, &entry.target)
            }
        } else if entry.target.is_file() {
            fs::remove_file(&entry.target)
                .map_err(|e| format!("删除 {} 失败: {e}", entry.target.display()))
        } else {
            Ok(())
        };
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn copy_replacing(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "目标文件名无效".to_string())?;
    let temporary = target.with_file_name(format!(".{name}.optinr.tmp"));
    let _ = fs::remove_file(&temporary);
    fs::copy(source, &temporary).map_err(|e| format!("复制 {} 失败: {e}", source.display()))?;
    if target.exists() {
        fs::remove_file(target).map_err(|e| format!("替换 {} 失败: {e}", target.display()))?;
    }
    fs::rename(&temporary, target).map_err(|e| format!("提交 {} 失败: {e}", target.display()))
}

fn record_target(game_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = safe_relative(Path::new(relative))?;
    Ok(game_dir.join(relative))
}
