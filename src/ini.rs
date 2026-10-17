use std::fs;
use std::path::Path;

use crate::util::write_atomic;

const REQUIRED: &[(&str, &str, &str)] = &[
    ("DlssNr", "Enabled", "true"),
    ("DlssNr", "RunBeforeSR", "true"),
    ("DlssNr", "NrBackend", "mochizuki"),
    ("Plugins", "LoadAsiPlugins", "true"),
    ("FrameGen", "External", "false"),
    ("FrameGen", "Enabled", "true"),
    ("FrameGen", "FGInput", "dlssg"),
    ("FrameGen", "FGOutput", "xefg"),
    ("XeFG", "InterpolationCount", "1"),
];

pub fn apply_fixed_profile(path: &Path, disable_frame_gen: bool) -> Result<Vec<String>, String> {
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
    for (section, key, value) in REQUIRED {
        if disable_frame_gen && *section == "FrameGen" && *key == "Enabled" {
            ensure_value(&mut lines, section, key, "false", &mut changes)?;
        } else {
            ensure_value(&mut lines, section, key, value, &mut changes)?;
        }
    }
    let mut encoded = Vec::new();
    if has_bom {
        encoded.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    encoded.extend_from_slice((lines.join("\r\n") + "\r\n").as_bytes());
    write_atomic(path, &encoded)?;
    Ok(changes)
}

fn ensure_value(
    lines: &mut Vec<String>,
    section: &str,
    key: &str,
    value: &str,
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
        if !old.eq_ignore_ascii_case(value) {
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
        ensure_value(&mut lines, "XeFG", "InterpolationCount", "1", &mut changes).unwrap();
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
        assert!(ensure_value(&mut lines, "FrameGen", "Enabled", "true", &mut Vec::new()).is_err());
    }
}
