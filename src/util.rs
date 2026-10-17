use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("解析 {} 失败: {e}", path.display()))
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("序列化 JSON 失败: {e}"))?;
    write_atomic(path, &bytes)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|v| v.to_str()).unwrap_or("file")
    ));
    {
        let mut file =
            File::create(&tmp).map_err(|e| format!("创建 {} 失败: {e}", tmp.display()))?;
        file.write_all(bytes)
            .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("同步 {} 失败: {e}", tmp.display()))?;
    }
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("替换 {} 失败: {e}", path.display()))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("提交 {} 失败: {e}", path.display()))
}

pub fn safe_relative(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err("压缩包包含绝对路径".to_string());
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(format!("压缩包包含不安全路径: {}", path.display())),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("压缩包包含空路径".to_string());
    }
    Ok(out)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn process_is_running(exe: &Path) -> Result<bool, String> {
    let name = exe
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| "游戏 EXE 名称无效".to_string())?;
    let output = hidden_command("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("检查游戏进程失败: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .to_ascii_lowercase()
        .contains(&name.to_ascii_lowercase()))
}

#[cfg(windows)]
pub fn hidden_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(windows))]
pub fn hidden_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

pub fn game_id(exe: &Path) -> String {
    let normalized = exe
        .canonicalize()
        .unwrap_or_else(|_| exe.to_path_buf())
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex_lower(&digest[..8])
}

pub fn file_contains_marker(path: &Path, marker: &str) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let ascii = marker.as_bytes();
    let utf16 = marker
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let overlap = (ascii.len().max(utf16.len()) - 1).min(256);
    let mut carry = Vec::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        if read == 0 {
            return Ok(false);
        }
        let mut window = Vec::with_capacity(carry.len() + read);
        window.extend_from_slice(&carry);
        window.extend_from_slice(&buf[..read]);
        if contains(&window, ascii) || contains(&window, &utf16) {
            return Ok(true);
        }
        carry.clear();
        let start = window.len().saturating_sub(overlap);
        carry.extend_from_slice(&window[start..]);
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

pub fn remove_tree_safely(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("检查 {} 失败: {e}", path.display()))?;
    if is_reparse(&metadata) {
        #[cfg(windows)]
        let directory = {
            use std::os::windows::fs::MetadataExt;
            // symlink_metadata().is_dir() is false for Windows directory junctions.
            metadata.file_attributes() & 0x10 != 0
        };
        #[cfg(not(windows))]
        let directory = metadata.is_dir();
        return if directory {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        }
        .map_err(|e| format!("删除重解析点 {} 失败: {e}", path.display()));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?
        {
            remove_tree_safely(
                &entry
                    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?
                    .path(),
            )?;
        }
        fs::remove_dir(path).map_err(|e| format!("删除 {} 失败: {e}", path.display()))
    } else {
        fs::remove_file(path).map_err(|e| format!("删除 {} 失败: {e}", path.display()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_archive_escape() {
        assert!(safe_relative(Path::new("../evil.dll")).is_err());
        assert!(safe_relative(Path::new("C:\\evil.dll")).is_err());
        assert_eq!(
            safe_relative(Path::new("OptiScaler/libxess.dll")).unwrap(),
            PathBuf::from("OptiScaler").join("libxess.dll")
        );
    }

    #[test]
    fn finds_ascii_and_utf16_markers() {
        assert!(contains(b"xxOptiScalerxx", b"OptiScaler"));
        let utf16 = "OptiScaler"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut data = b"prefix".to_vec();
        data.extend_from_slice(&utf16);
        assert!(contains(&data, &utf16));
    }
}
