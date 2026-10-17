use std::fs;
use std::path::Path;

use crate::util::write_atomic;

const CORE: &[(&str, &str, &str)] = &[
    ("DlssNr", "Enabled", "true"),
    ("DlssNr", "RunBeforeSR", "true"),
    ("DlssNr", "NrBackend", "mochizuki"),
    ("Plugins", "LoadAsiPlugins", "true"),
    ("FrameGen", "External", "false"),
    ("FrameGen", "FGInput", "dlssg"),
    ("FrameGen", "FGOutput", "xefg"),
];
const PREFERENCES: &[(&str, &str)] = &[("FrameGen", "Enabled"), ("XeFG", "InterpolationCount")];

pub fn apply_fixed_profile(
    path: &Path,
    disable_frame_gen: bool,
    preserve_preferences: bool,
) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let has_bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let text = std::str::from_utf8(if has_bom { &bytes[3..] } else { &bytes })
        .map_err(|e| format!("{} 不是 UTF-8: {e}", path.display()))?;
    let mut lines = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut changes = Vec::new();
    for (section, key, value) in CORE {
        ensure_value(&mut lines, section, key, value, true, &mut changes)?;
    }
    ensure_value(
        &mut lines,
        "FrameGen",
        "Enabled",
        if disable_frame_gen { "false" } else { "true" },
        !preserve_preferences || disable_frame_gen,
        &mut changes,
    )?;
    ensure_value(
        &mut lines,
        "XeFG",
        "InterpolationCount",
        "1",
        !preserve_preferences,
        &mut changes,
    )?;
    let mut encoded = Vec::new();
    if has_bom {
        encoded.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    encoded.extend_from_slice((lines.join("\r\n") + "\r\n").as_bytes());
    write_atomic(path, &encoded)?;
    Ok(changes)
}

pub fn check_fixed_profile(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map_err(|e| format!("{} 不是 UTF-8: {e}", path.display()))?;
    let normalized = text.replace("\r\n", "\n");
    let lines = normalized.split('\n').collect::<Vec<_>>();
    let mut issues = Vec::new();
    for (section, key, expected) in CORE {
        let Some((start, end)) = find_section_str(&lines, section) else {
            issues.push(format!("缺少配置段 [{section}]"));
            continue;
        };
        let values = lines[start + 1..end]
            .iter()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with(';') || line.starts_with('#') {
                    return None;
                }
                let (actual_key, value) = line.split_once('=')?;
                actual_key
                    .trim()
                    .eq_ignore_ascii_case(key)
                    .then(|| value.trim().to_string())
            })
            .collect::<Vec<_>>();
        if values.len() > 1 {
            issues.push(format!("[{section}] {key} 重复"));
        } else if values.is_empty() {
            issues.push(format!("缺少 [{section}] {key}"));
        } else if !values[0].eq_ignore_ascii_case(expected) {
            issues.push(format!("[{section}] {key}={}，期望 {expected}", values[0]));
        }
    }
    for (section, key) in PREFERENCES {
        let Some((start, end)) = find_section_str(&lines, section) else {
            issues.push(format!("缺少配置段 [{section}]"));
            continue;
        };
        let values = lines[start + 1..end]
            .iter()
            .filter_map(|line| {
                let line = line.trim();
                if line.starts_with(';') || line.starts_with('#') {
                    return None;
                }
                let (actual_key, value) = line.split_once('=')?;
                actual_key
                    .trim()
                    .eq_ignore_ascii_case(key)
                    .then(|| value.trim().to_string())
            })
            .collect::<Vec<_>>();
        if values.len() > 1 {
            issues.push(format!("[{section}] {key} 重复"));
        } else if values.is_empty() {
            issues.push(format!("缺少 [{section}] {key}"));
        } else if *section == "FrameGen"
            && !matches!(values[0].to_ascii_lowercase().as_str(), "true" | "false")
        {
            issues.push(format!("[{section}] {key} 不是 true/false"));
        } else if *section == "XeFG"
            && !values[0]
                .parse::<u32>()
                .is_ok_and(|count| (1..=5).contains(&count))
        {
            issues.push(format!("[{section}] {key} 不在 1..5"));
        }
    }
    Ok(issues)
}

fn ensure_value(
    lines: &mut Vec<String>,
    section: &str,
    key: &str,
    value: &str,
    overwrite: bool,
    changes: &mut Vec<String>,
) -> Result<(), String> {
    let Some((section_start, section_end)) = find_section(lines, section) else {
        lines.push(String::new());
        lines.push(format!("[{section}]"));
        lines.push(format!("{key}={value}"));
        changes.push(format!("新增 [{section}] {key}={value}"));
        return Ok(());
    };
    let mut matches = Vec::new();
    for (index, line) in lines
        .iter()
        .enumerate()
        .take(section_end)
        .skip(section_start + 1)
    {
        let trimmed = line.trim();
        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((actual_key, actual_value)) = trimmed.split_once('=') else {
            continue;
        };
        if actual_key.trim().eq_ignore_ascii_case(key) {
            matches.push((index, actual_value.trim().to_string()));
        }
    }
    if matches.len() > 1 {
        return Err(format!("[{section}] 下 {key} 出现多次，拒绝修改"));
    }
    if let Some((index, old)) = matches.pop() {
        if overwrite && !old.eq_ignore_ascii_case(value) {
            lines[index] = format!("{key}={value}");
            changes.push(format!("[{section}] {key}: {old} -> {value}"));
        }
        return Ok(());
    }
    let mut insert_at = section_end;
    while insert_at > section_start + 1
        && lines
            .get(insert_at - 1)
            .is_some_and(|line| line.trim().is_empty())
    {
        insert_at -= 1;
    }
    lines.insert(insert_at, format!("{key}={value}"));
    changes.push(format!("新增 [{section}] {key}={value}"));
    Ok(())
}

fn find_section(lines: &[String], wanted: &str) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && trimmed[1..trimmed.len() - 1]
                .trim()
                .eq_ignore_ascii_case(wanted)
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    Some((start, end))
}

fn find_section_str(lines: &[&str], wanted: &str) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && trimmed[1..trimmed.len() - 1]
                .trim()
                .eq_ignore_ascii_case(wanted)
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_only_exact_section_and_key() {
        let mut lines = vec![
            "[DLSSG]".into(),
            "InterpolationCount=5".into(),
            "[XeFG]".into(),
            "; keep me".into(),
            "InterpolationCount=4".into(),
            "HighResMV=true".into(),
        ];
        let mut changes = Vec::new();
        ensure_value(
            &mut lines,
            "XeFG",
            "InterpolationCount",
            "1",
            true,
            &mut changes,
        )
        .unwrap();
        assert_eq!(lines[1], "InterpolationCount=5");
        assert_eq!(lines[4], "InterpolationCount=1");
        assert_eq!(lines[5], "HighResMV=true");
    }

    #[test]
    fn rejects_duplicate_key() {
        let mut lines = vec![
            "[FrameGen]".into(),
            "Enabled=false".into(),
            "Enabled=true".into(),
        ];
        assert!(
            ensure_value(
                &mut lines,
                "FrameGen",
                "Enabled",
                "true",
                true,
                &mut Vec::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn check_ignores_unrelated_user_values() {
        let dir = test_dir("unrelated");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("OptiScaler.ini");
        fs::write(
            &path,
            "[DlssNr]\nEnabled=true\nRunBeforeSR=true\nNrBackend=mochizuki\n\
             [Plugins]\nLoadAsiPlugins=true\n\
             [FrameGen]\nExternal=false\nEnabled=true\nFGInput=dlssg\nFGOutput=xefg\n\
             [XeFG]\nInterpolationCount=1\nHighResMV=true\n",
        )
        .unwrap();
        assert!(check_fixed_profile(&path).unwrap().is_empty());
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn maintenance_preserves_user_frame_generation_preferences() {
        let dir = test_dir("preferences");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("OptiScaler.ini");
        fs::write(
            &path,
            "[DlssNr]\nEnabled=true\nRunBeforeSR=true\nNrBackend=mochizuki\n\
             [Plugins]\nLoadAsiPlugins=true\n\
             [FrameGen]\nExternal=false\nEnabled=false\nFGInput=dlssg\nFGOutput=xefg\n\
             [XeFG]\nInterpolationCount=3\nHighResMV=true\n",
        )
        .unwrap();
        apply_fixed_profile(&path, false, true).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Enabled=false"));
        assert!(text.contains("InterpolationCount=3"));
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("optinr-ini-{name}-{}-{nonce}", std::process::id()))
    }
}
