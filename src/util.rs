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
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("检查游戏进程失败: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .to_ascii_lowercase()
        .contains(&name.to_ascii_lowercase()))
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
}
