use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
    let existing_record = if record_path.is_file() {
        Some(
            read_json::<InstallRecord>(&record_path)
                .map_err(|error| format!("安装记录损坏，拒绝覆盖：{error}"))?,
        )
    } else {
        None
    };
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
    let overwrite_count = count_existing_targets(&game.game_dir, &plan, &ini_path)?;
    if overwrite_count > 0 {
        progress(Progress::new(
            "预检查",
            format!("发现 {overwrite_count} 个同名文件，将直接覆盖"),
            Some(0.04),
        ));
    }

    let mut cleanup_files = BTreeMap::new();
    if let Some(previous) = existing_record.as_ref() {
        for file in &previous.files {
            cleanup_files.insert(file.relative_path.to_ascii_lowercase(), file.clone());
        }
    }
    for file in &plan {
        cleanup_files.insert(
            file.relative.to_ascii_lowercase(),
            ManagedFile {
                relative_path: file.relative.clone(),
                sha256: file.sha256.clone(),
            },
        );
    }
    cleanup_files
        .entry("optiscaler.ini".to_string())
        .or_insert_with(|| ManagedFile {
            relative_path: "OptiScaler.ini".to_string(),
            sha256: String::new(),
        });
    let partial_record = InstallRecord {
        schema: 2,
        game_name: game.name.clone(),
        exe: game.exe.clone(),
        installed_at_ms: now_ms(),
        complete: false,
        upstream_tag: components.upstream_tag.clone(),
        extras_version: Some(components.extras_version.clone()),
        profile: crate::model::FIXED_PROFILE.to_string(),
        proxy_file: proxy.clone(),
        files: cleanup_files.into_values().collect(),
    };
    write_json_atomic(&record_path, &partial_record)?;

    progress(Progress::new(
        "清理",
        "移除旧版本受管文件，不保留游戏文件备份",
        Some(0.08),
    ));
    let result = (|| {
        progress(Progress::new(
            "清理",
            "移除已有的注入文件和旧版本残留",
            Some(0.10),
        ));
        if let Some(previous) = existing_record.as_ref() {
            remove_record_files(&game.game_dir, previous, true)?;
        }
        prune_injection_dirs(&game.game_dir);
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
            complete: true,
            upstream_tag: components.upstream_tag.clone(),
            extras_version: Some(components.extras_version.clone()),
            profile: crate::model::FIXED_PROFILE.to_string(),
            proxy_file: proxy.clone(),
            files: managed,
        };
        write_json_atomic(&record_path, &record)?;
        let mut report = MaintenanceReport {
            title: "安装完成".to_string(),
            lines: vec![
                format!("游戏：{}", game.name),
                format!("组件：{}", components.upstream_tag),
                format!("代理：{proxy}"),
                format!("固定资源：{}", components.extras_version),
                format!("受管文件：{} 个", record.files.len()),
                format!("覆盖同名文件：{overwrite_count} 个；卸载时按安装记录删除"),
            ],
        };
        if let Err(error) = cleanup_installer_files(paths) {
            report
                .lines
                .push(format!("安装已完成，但组件缓存清理失败：{error}"));
        }
        Ok(report)
    })();

    match result {
        Ok(report) => Ok(report),
        Err(error) => Err(format!(
            "{error}\n安装未完成，未执行自动回滚；请先在维护页卸载，再重新安装。"
        )),
    }
}

pub fn check_install(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    let record = load_record(paths, game)?;
    if !record.complete {
        return Ok(MaintenanceReport {
            title: "上次安装未完成".to_string(),
            lines: vec!["请先卸载该游戏，再重新安装".to_string()],
        });
    }
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

pub fn install_status(paths: &AppPaths, game: &GameInfo) -> Option<String> {
    read_json::<InstallRecord>(&paths.games.join(game_id(&game.exe)).join("record.json"))
        .ok()
        .map(|record| {
            if record.complete {
                format!("已安装 {}", record.upstream_tag)
            } else {
                "安装未完成".to_string()
            }
        })
}

pub fn uninstall(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再卸载".to_string());
    }
    let record = load_record(paths, game)?;
    let removed = remove_record_files(&game.game_dir, &record, false)?;
    let state_dir = paths.games.join(game_id(&game.exe));
    let _ = fs::remove_file(state_dir.join("record.json"));
    prune_injection_dirs(&game.game_dir);
    let mut report = MaintenanceReport {
        title: "卸载完成".to_string(),
        lines: vec![
            format!("删除 {removed} 个注入文件"),
            "游戏已恢复到无本项目注入的状态，未保留旧版本备份".to_string(),
        ],
    };
    if let Err(error) = cleanup_installer_files(paths) {
        report
            .lines
            .push(format!("卸载已完成，但组件缓存清理失败：{error}"));
    }
    Ok(report)
}

pub fn disable_frame_gen(paths: &AppPaths, game: &GameInfo) -> Result<MaintenanceReport, String> {
    if process_is_running(&game.exe)? {
        return Err("游戏正在运行，请完全退出后再修改".to_string());
    }
    let mut record = load_record(paths, game)?;
    if !record.complete {
        return Err("上次安装未完成，请先卸载后重新安装".to_string());
    }
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

pub fn cleanup_installer_files(paths: &AppPaths) -> Result<(), String> {
    clear_staging(paths)?;
    cleanup_self_delete_artifacts()?;
    for path in [
        paths.cache.clone(),
        paths.legacy_backups.clone(),
        paths.root.join("catalog"),
    ] {
        remove_if_exists(&path)?;
    }
    if let Ok(entries) = fs::read_dir(&paths.updates) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name == "previous.json"
                || name.ends_with(".part")
                || name.starts_with("previous-") && name.ends_with(".exe")
                || name.starts_with("optiscalernr-") && name.ends_with(".exe")
            {
                remove_if_exists(&path)?;
            }
        }
    }
    if let Ok(entries) = fs::read_dir(&paths.games) {
        for entry in entries.flatten() {
            let state_dir = entry.path();
            for name in ["transaction.json", "previous-record.json"] {
                remove_if_exists(&state_dir.join(name))?;
            }
        }
    }
    Ok(())
}

fn cleanup_self_delete_artifacts() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("读取当前程序路径失败: {e}"))?;
    let Some(dir) = exe.parent() else {
        return Ok(());
    };
    let Some(exe_name) = exe.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    for entry in fs::read_dir(dir)
        .map_err(|e| format!("读取程序目录 {} 失败: {e}", dir.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_self_delete_artifact(&name, exe_name) {
            remove_tree_safely(&entry.path())?;
        }
    }
    Ok(())
}

fn is_self_delete_artifact(name: &str, exe_name: &str) -> bool {
    let stem = exe_name
        .strip_suffix(".exe")
        .or_else(|| exe_name.strip_suffix(".EXE"))
        .unwrap_or(exe_name);
    name.starts_with(&format!(".{stem}.")) && name.ends_with(".__selfdelete__.exe")
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

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => remove_tree_safely(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("检查 {} 失败: {error}", path.display())),
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
    let path = paths.games.join(game_id(&game.exe)).join("record.json");
    if !path.is_file() {
        return Err("这个游戏还没有本程序的安装记录".to_string());
    }
    read_json(&path).map_err(|error| format!("安装记录损坏: {error}"))
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

fn count_existing_targets(
    game_dir: &Path,
    plan: &[PlannedFile],
    ini_path: &Path,
) -> Result<usize, String> {
    let mut count = 0;
    for file in plan {
        let target = game_dir.join(&file.relative);
        ensure_no_reparse_between(game_dir, &target)?;
        if target.exists() {
            if !target.is_file() {
                return Err(format!("目标不是普通文件: {}", target.display()));
            }
            count += 1;
        }
    }
    if ini_path.exists() {
        ensure_no_reparse_between(game_dir, ini_path)?;
        if !ini_path.is_file() {
            return Err(format!("目标不是普通文件: {}", ini_path.display()));
        }
        if !plan
            .iter()
            .any(|file| file.relative.eq_ignore_ascii_case("OptiScaler.ini"))
        {
            count += 1;
        }
    }
    Ok(count)
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
        if let Some(name) = target.file_name().and_then(|name| name.to_str()) {
            let temporary = target.with_file_name(format!(".{name}.optinr.tmp"));
            if temporary.is_file() {
                fs::remove_file(&temporary)
                    .map_err(|e| format!("删除 {} 失败: {e}", temporary.display()))?;
            }
        }
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
            complete: true,
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
        fs::write(game.join(".OptiScaler.dll.optinr.tmp"), "temporary").unwrap();
        fs::write(game.join("OptiScaler.ini"), "config").unwrap();
        fs::create_dir_all(game.join("OptiScaler").join("plugins")).unwrap();
        fs::write(
            game.join("OptiScaler").join("plugins").join("managed.dll"),
            "injected",
        )
        .unwrap();
        fs::write(
            game.join("OptiScaler").join("plugins").join("user.asi"),
            "user",
        )
        .unwrap();
        fs::write(game.join("Game.exe"), "game").unwrap();
        let install = record(&[
            "OptiScaler.dll",
            "OptiScaler.ini",
            "OptiScaler/plugins/managed.dll",
        ]);

        assert_eq!(remove_record_files(&game, &install, false).unwrap(), 3);
        assert!(!game.join("OptiScaler.dll").exists());
        assert!(!game.join(".OptiScaler.dll.optinr.tmp").exists());
        assert!(
            !game
                .join("OptiScaler")
                .join("plugins")
                .join("managed.dll")
                .exists()
        );
        assert!(
            game.join("OptiScaler")
                .join("plugins")
                .join("user.asi")
                .is_file()
        );
        assert!(game.join("Game.exe").is_file());
        prune_injection_dirs(&game);
        assert!(game.join("OptiScaler").join("plugins").is_dir());
        fs::remove_file(game.join("OptiScaler").join("plugins").join("user.asi")).unwrap();
        prune_injection_dirs(&game);
        assert!(!game.join("OptiScaler").exists());
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn install_counts_existing_same_name_files_for_notice() {
        let game = test_dir("overwrite-notice");
        fs::write(game.join("OptiScaler.dll"), "original").unwrap();
        fs::write(game.join("OptiScaler.ini"), "original").unwrap();
        let plan = [PlannedFile {
            source: game.join("source"),
            relative: "OptiScaler.dll".to_string(),
            sha256: String::new(),
        }];
        assert_eq!(
            count_existing_targets(&game, &plan, &game.join("OptiScaler.ini")).unwrap(),
            2
        );
        assert_eq!(
            fs::read_to_string(game.join("OptiScaler.dll")).unwrap(),
            "original"
        );
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn recognizes_only_installer_self_delete_artifacts() {
        assert!(is_self_delete_artifact(
            ".OptiScalerNR.abc.__selfdelete__.exe",
            "OptiScalerNR.exe"
        ));
        assert!(!is_self_delete_artifact(
            ".Other.abc.__selfdelete__.exe",
            "OptiScalerNR.exe"
        ));
        assert!(!is_self_delete_artifact(
            "OptiScalerNR.abc.__selfdelete__.exe",
            "OptiScalerNR.exe"
        ));
    }

    #[test]
    fn cleanup_keeps_only_records_and_catalog_metadata() {
        let root = test_dir("cleanup");
        let cache = root.join("cache");
        let staging = root.join("staging");
        let games = root.join("games");
        let backups = root.join("backups");
        let updates = root.join("updates");
        let catalog = root.join("catalog");
        fs::create_dir_all(cache.join("upstream").join("1")).unwrap();
        fs::create_dir_all(cache.join("extras")).unwrap();
        fs::create_dir_all(staging.join("upstream")).unwrap();
        fs::create_dir_all(games.join("game")).unwrap();
        fs::create_dir_all(backups.join("game").join("snapshot")).unwrap();
        fs::create_dir_all(updates.clone()).unwrap();
        fs::create_dir_all(catalog.join("old")).unwrap();
        fs::write(
            cache.join("upstream").join("1").join("package.zip"),
            "cache",
        )
        .unwrap();
        fs::write(cache.join("extras").join("extras-v1.zip"), "cache").unwrap();
        fs::write(staging.join("upstream").join("file"), "cache").unwrap();
        fs::write(games.join("game").join("record.json"), "{}").unwrap();
        fs::write(games.join("game").join("transaction.json"), "{}").unwrap();
        fs::write(games.join("game").join("previous-record.json"), "{}").unwrap();
        fs::write(backups.join("game").join("snapshot").join("file"), "old").unwrap();
        fs::write(catalog.join("old").join("file"), "old").unwrap();
        fs::write(updates.join("catalog-cache.json"), "{}").unwrap();
        for name in [
            "OptiScalerNR-0.2.0.exe",
            "previous-0.1.5.exe",
            "previous.json",
            "update.part",
        ] {
            fs::write(updates.join(name), "old").unwrap();
        }
        let paths = AppPaths {
            cache: cache.clone(),
            staging: staging.clone(),
            legacy_backups: backups.clone(),
            games: games.clone(),
            updates: updates.clone(),
            settings: root.join("settings.json"),
            root: root.clone(),
        };

        cleanup_installer_files(&paths).unwrap();
        assert!(!cache.exists());
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
        assert!(!backups.exists());
        assert!(!catalog.exists());
        assert!(updates.join("catalog-cache.json").is_file());
        for name in [
            "OptiScalerNR-0.2.0.exe",
            "previous-0.1.5.exe",
            "previous.json",
            "update.part",
        ] {
            assert!(!updates.join(name).exists());
        }
        assert!(games.join("game").join("record.json").is_file());
        assert!(!games.join("game").join("transaction.json").exists());
        assert!(!games.join("game").join("previous-record.json").exists());
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
            root: root.clone(),
        };

        assert_eq!(clear_staging(&paths).unwrap(), 1);
        assert!(!staging.join("upstream").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
