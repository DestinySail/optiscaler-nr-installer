use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::model::Progress;
use crate::util::safe_relative;

const MAX_FILES: usize = 20_000;
const MAX_UNCOMPRESSED: u64 = 4 * 1024 * 1024 * 1024;

pub fn extract_zip(
    zip_path: &Path,
    destination: &Path,
    progress_name: &str,
    mut progress: impl FnMut(Progress),
) -> Result<(), String> {
    let file =
        File::open(zip_path).map_err(|e| format!("打开压缩包 {} 失败: {e}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("读取压缩包失败: {e}"))?;
    if archive.len() > MAX_FILES {
        return Err(format!("压缩包文件数 {} 超过限制", archive.len()));
    }
    let total_entries = archive.len();
    fs::create_dir_all(destination)
        .map_err(|e| format!("创建解压目录 {} 失败: {e}", destination.display()))?;
    let mut seen = HashSet::new();
    let mut written = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("读取压缩包第 {index} 项失败: {e}"))?;
        let entry_name = entry
            .name()
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| format!("index {index}"));
        if entry.is_symlink() {
            return Err(format!("压缩包包含符号链接: {entry_name}"));
        }
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("压缩包包含不安全路径: {entry_name}"))?;
        let relative = safe_relative(&relative)?;
        let key = relative.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            return Err(format!("压缩包包含大小写冲突路径: {}", relative.display()));
        }
        let target = destination.join(&relative);
        if entry.is_dir() {
            ensure_safe_parent(destination, &target)?;
            fs::create_dir_all(&target)
                .map_err(|e| format!("创建目录 {} 失败: {e}", target.display()))?;
            continue;
        }
        written = written
            .checked_add(entry.size())
            .ok_or_else(|| "压缩包解压大小溢出".to_string())?;
        if written > MAX_UNCOMPRESSED {
            return Err("压缩包解压后超过 4 GiB 限制".to_string());
        }
        ensure_safe_parent(destination, &target)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        let mut output =
            File::create(&target).map_err(|e| format!("创建 {} 失败: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|e| format!("解压 {} 失败: {e}", relative.display()))?;
        output
            .sync_all()
            .map_err(|e| format!("同步 {} 失败: {e}", target.display()))?;
        if index % 250 == 0 || index + 1 == total_entries {
            progress(Progress::new(
                progress_name,
                format!(
                    "解压 {}/{}: {}",
                    index + 1,
                    total_entries,
                    relative.display()
                ),
                Some((index + 1) as f32 / total_entries as f32),
            ));
        }
    }
    Ok(())
}

pub fn find_release_root(root: &Path) -> Result<PathBuf, String> {
    find_marker_root(root, &["OptiScaler.dll", "OptiScaler.ini"], 3)
        .ok_or_else(|| format!("压缩包里找不到 OptiScaler 发布目录: {}", root.display()))
}

fn find_marker_root(root: &Path, markers: &[&str], depth: usize) -> Option<PathBuf> {
    if markers.iter().all(|marker| root.join(marker).is_file()) {
        return Some(root.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        if entry.file_type().ok()?.is_dir()
            && let Some(found) = find_marker_root(&entry.path(), markers, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn ensure_safe_parent(base: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(base)
        .map_err(|_| "解压路径逃出目标目录".to_string())?;
    let mut cursor = base.to_path_buf();
    for component in relative.components() {
        cursor.push(component);
        if cursor
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(format!("目标目录包含重解析点: {}", cursor.display()));
        }
    }
    Ok(())
}

pub fn read_small_text(path: &Path, max: u64) -> Result<String, String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("读取 {} 信息失败: {e}", path.display()))?;
    if metadata.len() > max {
        return Err(format!("{} 超过大小限制", path.display()));
    }
    let mut text = String::new();
    File::open(path)
        .map_err(|e| format!("打开 {} 失败: {e}", path.display()))?
        .read_to_string(&mut text)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    Ok(text)
}
