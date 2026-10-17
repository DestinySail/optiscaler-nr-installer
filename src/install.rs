use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets::PreparedComponents;
use crate::ini;
use crate::model::{AppPaths, GameInfo, InstallRecord, MaintenanceReport, ManagedFile, Progress};
use crate::util::{
    file_contains_marker, game_id, now_ms, process_is_running, read_json, remove_tree_safely,
    safe_relative, sha256_file, write_json_atomic,
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
    #[serde(default)]
    backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Transaction {
    schema: u32,
    game_dir: PathBuf,
    #[serde(default)]
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
    let ini_existed = ini_path.is_file();
    let template_ini = components.release_root.join("OptiScaler.ini");

    progress(Progress::new("预检查", "生成固定方案文件计划", Some(0.02)));
    let mut plan = build_plan(components, &proxy)?;
    if ini_existed {
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

    progress(Progress::new(
        "清理",
        "检查现有文件，不保留游戏文件备份",
        Some(0.08),
    ));
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file in &plan {
        let target = game.game_dir.join(&file.relative);
        add_transaction_entry(&game.game_dir, &target, &mut entries, &mut seen)?;
    }
    add_transaction_entry(&game.game_dir, &ini_path, &mut entries, &mut seen)?;
    if let Some(previous) = existing_record.as_ref() {
        for file in &previous.files {
            let target = record_target(&game.game_dir, &file.relative_path)?;
            add_transaction_entry(&game.game_dir, &target, &mut entries, &mut seen)?;
        }
    }
    let transaction_path = state_dir.join("transaction.json");
    let transaction = Transaction {
        schema: 2,
        game_dir: game.game_dir.clone(),
        backup_root: PathBuf::new(),
        entries: entries.clone(),
        started_at_ms: now_ms(),
    };
    write_json_atomic(&transaction_path, &transaction)?;

    let result = (|| {
        progress(Progress::new(
            "清理",
            "移除已有的注入文件和旧版本残留",
            Some(0.10),
        ));
        if let Some(previous) = existing_record.as_ref() {
            remove_record_files(&game.game_dir, previous, true)?;
        }
        remove_injection_dirs(&game.game_dir)?;
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
        ini::apply_fixed_profile(&ini_path, false, ini_existed)?;

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
            progress(Progress::new(
                "提交",
                format!("从 {} 升级", previous.upstream_tag),
                Some(0.98),
            ));
        }
        let record = InstallRecord {
            schema: 2,
            game_name: game.name.clone(),
            exe: game.exe.clone(),
            installed_at_ms: now_ms(),
            upstream_tag: components.upstream_tag.clone(),
            extras_version: Some(components.extras_version.clone()),
            profile: crate::model::FIXED_PROFILE.to_string(),
            proxy_file: proxy.clone(),
            files: managed,
        };
        write_json_atomic(&record_path, &record)?;
        let _ = fs::remove_file(&previous_record_path);
        fs::remove_file(&transaction_path).map_err(|e| format!("提交事务记录失败: {e}"))?;
        Ok(MaintenanceReport {
            title: "安装完成".to_string(),
            lines: vec![
                format!("游戏：{}", game.name),
                format!("组件：{}", components.upstream_tag),
                format!("代理：{proxy}"),
                format!("固定资源：{}", components.extras_version),
                format!("受管文件：{} 个", record.files.len()),
                "不保留游戏文件备份；卸载会删除本项目注入的文件".to_string(),
            ],
        })
    })();

    if let Err(error) = &result {
        return match rollback_transaction(&transaction) {
            Ok(()) => {
                let _ = fs::remove_file(&transaction_path);
                let _ = fs::remove_file(&record_path);
                let _ = fs::remove_file(&previous_record_path);
                prune_injection_dirs(&game.game_dir);
                Err(format!(
                    "{error}\n已移除本次注入的文件，游戏恢复到无本项目状态。"
                ))
            }
            Err(rollback_error) => Err(format!(
                "{error}\n自动清理未完成：{rollback_error}\n事务日志已保留，下次启动会继续清理。"
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
                ini::check_fixed_profile(&target)?
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

pub fn installed_tag(paths: &AppPaths, game: &GameInfo) -> Option<String> {
    read_json::<InstallRecord>(&paths.games.join(game_id(&game.exe)).join("record.json"))
        .ok()
        .map(|record| record.upstream_tag)
}

pub fn uninstall(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再卸载".to_string());
    }
    let record = load_record(paths, game)?;
    let removed = remove_record_files(&game.game_dir, &record, false)?;
    let state_dir = paths.games.join(game_id(&game.exe));
    let _ = fs::remove_file(state_dir.join("record.json"));
    let _ = fs::remove_file(state_dir.join("previous-record.json"));
    prune_injection_dirs(&game.game_dir);
    Ok(MaintenanceReport {
        title: "卸载完成".to_string(),
        lines: vec![
            format!("删除 {removed} 个注入文件"),
            "游戏已恢复到无本项目注入的状态，未保留旧版本备份".to_string(),
        ],
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
    ini::apply_fixed_profile(&ini_path, true, true)?;
    if let Some(ini_entry) = record
        .files
        .iter_mut()
        .find(|file| file.relative_path.eq_ignore_ascii_case("OptiScaler.ini"))
    {
        ini_entry.sha256 = sha256_file(&ini_path)?;
    }
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

pub fn recover_transactions(paths: &AppPaths) -> Vec<String> {
    let mut messages = Vec::new();
    if let Ok(entries) = fs::read_dir(&paths.games) {
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
                        "已清理上次未完成事务：{}",
                        transaction.game_dir.display()
                    ));
                }
                Err(error) => messages.push(format!(
                    "未完成事务恢复失败 {}: {error}",
                    transaction.game_dir.display()
                )),
            }
        }
    }
    if let Some(message) = discard_legacy_backups(paths) {
        messages.push(message);
    }
    match clear_staging(paths) {
        Ok(0) => {}
        Ok(files) => messages.push(format!("已清理组件临时目录：{files} 个文件")),
        Err(error) => messages.push(format!("清理组件临时目录失败: {error}")),
    }
    messages
}

pub fn clear_staging(paths: &AppPaths) -> Result<usize, String> {
    let files = count_files(&paths.staging);
    let entries = match fs::read_dir(&paths.staging) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("读取 {} 失败: {error}", paths.staging.display())),
    };
    for entry in entries {
        let path = entry
            .map_err(|e| format!("读取 {} 失败: {e}", paths.staging.display()))?
            .path();
        remove_tree_safely(&path)?;
    }
    Ok(files)
}

fn discard_legacy_backups(paths: &AppPaths) -> Option<String> {
    if !paths.legacy_backups.exists() {
        return None;
    }
    let pending_transaction = fs::read_dir(&paths.games).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.path().join("transaction.json").is_file())
    });
    if pending_transaction {
        return Some("存在未完成事务，旧版本备份暂未删除".to_string());
    }
    let files = count_files(&paths.legacy_backups);
    match remove_tree_safely(&paths.legacy_backups) {
        Ok(()) => Some(format!("已删除旧版本本地备份：{files} 个文件")),
        Err(error) => Some(format!("删除旧版本本地备份失败: {error}")),
    }
}

fn count_files(root: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    count
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
    target: &Path,
    entries: &mut Vec<TransactionEntry>,
    seen: &mut HashSet<String>,
) -> Result<(), String> {
    ensure_no_reparse_between(game_dir, target)?;
    if target.exists() && !target.is_file() {
        return Err(format!("目标不是普通文件: {}", target.display()));
    }
    let relative = target
        .strip_prefix(game_dir)
        .map_err(|_| format!("目标不在游戏目录内: {}", target.display()))?;
    let key = relative.to_string_lossy().to_ascii_lowercase();
    if !seen.insert(key) {
        return Ok(());
    }
    entries.push(TransactionEntry {
        target: target.to_path_buf(),
        backup: None,
    });
    Ok(())
}

fn remove_record_files(
    game_dir: &Path,
    record: &InstallRecord,
    keep_ini: bool,
) -> Result<usize, String> {
    let mut removed = 0;
    for file in &record.files {
        if keep_ini && file.relative_path.eq_ignore_ascii_case("OptiScaler.ini") {
            continue;
        }
        let target = record_target(game_dir, &file.relative_path)?;
        ensure_no_reparse_between(game_dir, &target)?;
        if target.is_file() {
            fs::remove_file(&target).map_err(|e| format!("删除 {} 失败: {e}", target.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn prune_injection_dirs(game_dir: &Path) {
    for relative in ["OptiScaler", "dlssnr-amd", "person-model"] {
        remove_empty_dirs(game_dir.join(relative));
    }
}

fn remove_injection_dirs(game_dir: &Path) -> Result<(), String> {
    for relative in ["OptiScaler", "dlssnr-amd", "person-model"] {
        let path = game_dir.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("检查 {} 失败: {error}", path.display())),
        };
        if metadata.file_type().is_symlink() {
            return Err(format!("拒绝清理重解析点: {}", path.display()));
        }
        if metadata.is_dir() {
            remove_tree_safely(&path)?;
        } else {
            fs::remove_file(&path).map_err(|e| format!("清理 {} 失败: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn remove_empty_dirs(path: PathBuf) {
    if fs::symlink_metadata(&path).is_ok_and(|meta| is_reparse(&meta)) {
        return;
    }
    let Ok(entries) = fs::read_dir(&path) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_empty_dirs(entry.path());
        }
    }
    let _ = fs::remove_dir(&path);
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
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
        if let Err(error) = ensure_no_reparse_between(&transaction.game_dir, &entry.target) {
            errors.push(error);
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
    prune_injection_dirs(&transaction.game_dir);
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

fn ensure_no_reparse_between(base: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(base)
        .map_err(|_| format!("目标不在游戏目录内: {}", target.display()))?;
    let mut cursor = base.to_path_buf();
    for component in relative.components() {
        cursor.push(component);
        match cursor.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("目标路径包含重解析点: {}", cursor.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(format!("检查 {} 失败: {error}", cursor.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("optinr-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn record(files: &[&str]) -> InstallRecord {
        InstallRecord {
            schema: 2,
            game_name: "Test".to_string(),
            exe: PathBuf::from("Test.exe"),
            installed_at_ms: 0,
            upstream_tag: "1".to_string(),
            extras_version: None,
            profile: "test".to_string(),
            proxy_file: "dxgi.dll".to_string(),
            files: files
                .iter()
                .map(|path| ManagedFile {
                    relative_path: (*path).to_string(),
                    sha256: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn uninstall_removes_managed_files_and_keeps_game_files() {
        let game = test_dir("uninstall");
        fs::write(game.join("OptiScaler.dll"), "injected").unwrap();
        fs::write(game.join("OptiScaler.ini"), "config").unwrap();
        fs::write(game.join("Game.exe"), "game").unwrap();
        let install = record(&["OptiScaler.dll", "OptiScaler.ini"]);

        assert_eq!(remove_record_files(&game, &install, false).unwrap(), 2);
        assert!(!game.join("OptiScaler.dll").exists());
        assert!(game.join("Game.exe").is_file());
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn failed_transaction_removes_new_injection() {
        let game = test_dir("transaction");
        let target = game.join("OptiScaler.dll");
        fs::write(&target, "injected").unwrap();
        let transaction = Transaction {
            schema: 2,
            game_dir: game.clone(),
            backup_root: PathBuf::new(),
            entries: vec![TransactionEntry {
                target: target.clone(),
                backup: None,
            }],
            started_at_ms: 1,
        };

        rollback_transaction(&transaction).unwrap();
        assert!(!target.exists());
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn removes_legacy_backup_tree() {
        let root = test_dir("legacy");
        let games = root.join("games");
        let backups = root.join("backups");
        fs::create_dir_all(&games).unwrap();
        fs::create_dir_all(backups.join("game").join("snapshot")).unwrap();
        fs::write(backups.join("game").join("snapshot").join("file"), "old").unwrap();
        let paths = AppPaths {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: backups.clone(),
            games,
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            upstream_check: root.join("catalog").join("upstream.json"),
            root: root.clone(),
        };

        assert!(discard_legacy_backups(&paths).is_some());
        assert!(!backups.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clears_derived_staging_tree() {
        let root = test_dir("staging");
        let staging = root.join("staging");
        fs::create_dir_all(staging.join("upstream").join("nested")).unwrap();
        fs::write(
            staging.join("upstream").join("nested").join("file"),
            "cache",
        )
        .unwrap();
        let paths = AppPaths {
            cache: root.join("cache"),
            staging: staging.clone(),
            legacy_backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            upstream_check: root.join("catalog").join("upstream.json"),
            root: root.clone(),
        };

        assert_eq!(clear_staging(&paths).unwrap(), 1);
        assert!(!staging.join("upstream").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
