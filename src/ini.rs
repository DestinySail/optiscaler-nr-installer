use std::fs;
use std::path::Path;

use crate::model::ModelSelection;
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
    models: Option<ModelSelection>,
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
    if let Some(models) = models {
        let retained = find_section(&lines, "DlssNr").and_then(|(start, end)| {
            lines[start + 1..end].iter().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim().eq_ignore_ascii_case("MochizukiNetworkModel")
                    && matches!(value.trim(), "0" | "1"))
                .then(|| value.trim().to_string())
            })
        });
        let value = if models == ModelSelection::Both && preserve_preferences {
            retained.as_deref().unwrap_or("1")
        } else if models == ModelSelection::Original {
            "0"
        } else {
            "1"
        };
        ensure_value(
            &mut lines,
            "DlssNr",
            "MochizukiNetworkModel",
            value,
            true,
            &mut changes,
        )?;
    }
    let mut encoded = Vec::new();
    if has_bom {
        encoded.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    encoded.extend_from_slice((lines.join("\r\n") + "\r\n").as_bytes());
    write_atomic(path, &encoded)?;
    Ok(changes)
}

pub fn check_fixed_profile(
    path: &Path,
    models: Option<ModelSelection>,
) -> Result<Vec<String>, String> {
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
    let mut preferences = PREFERENCES.to_vec();
    if models.is_some() {
        preferences.push(("DlssNr", "MochizukiNetworkModel"));
    }
    for (section, key) in &preferences {
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
            if *key != "MochizukiNetworkModel" || models != Some(ModelSelection::Original) {
                issues.push(format!("缺少 [{section}] {key}"));
            }
        } else if *key == "MochizukiNetworkModel"
            && !matches!(
                (models, values[0].as_str()),
                (Some(ModelSelection::Original | ModelSelection::Both), "0")
                    | (Some(ModelSelection::Small | ModelSelection::Both), "1")
            )
        {
            issues.push(format!(
                "[{section}] {key}={}，与已安装模型不匹配",
                values[0]
            ));
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

pub fn apply_asi_profile(
    game_dir: &Path,
    release_root: &Path,
    models: ModelSelection,
) -> Result<(), String> {
    let path = game_dir.join("dlssnr-amd.ini");
    if !path.is_file() {
        let template = [
            "dlssnr-amd-optiscaler-renodx.ini",
            "dlssnr-amd-optiscaler.ini",
        ]
        .iter()
        .map(|name| release_root.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "主组件缺少 dlssnr-amd.ini 模板".to_string())?;
        fs::copy(&template, &path).map_err(|e| format!("复制 {} 失败: {e}", template.display()))?;
    }
    if let Some(model) = models.asi_model() {
        let mut lines = read_ini_lines(&path)?;
        ensure_value(&mut lines, "Network", "Model", model, true, &mut Vec::new())?;
        write_atomic(&path, (lines.join("\r\n") + "\r\n").as_bytes())?;
    }
    let mirror = game_dir.join("dlssnr-amd").join("dlssnr-amd.default.ini");
    if let Some(parent) = mirror.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    fs::copy(&path, &mirror).map_err(|e| format!("复制 {} 失败: {e}", mirror.display()))?;
    Ok(())
}

fn read_ini_lines(path: &Path) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map_err(|e| format!("{} 不是 UTF-8: {e}", path.display()))?;
    let mut lines = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Ok(lines)
}

pub fn check_asi_profile(game_dir: &Path, models: ModelSelection) -> Result<Vec<String>, String> {
    let path = game_dir.join("dlssnr-amd.ini");
    if !path.is_file() {
        return Ok(vec!["缺少 dlssnr-amd.ini".to_string()]);
    }
    let lines = read_ini_lines(&path)?;
    let lines = lines.iter().map(String::as_str).collect::<Vec<_>>();
    let Some((start, end)) = find_section_str(&lines, "Network") else {
        return Ok(vec!["缺少配置段 [Network]".to_string()]);
    };
    let value = lines[start + 1..end].iter().find_map(|line| {
        let line = line.trim();
        if line.starts_with(';') || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("Model")
            .then(|| value.trim().to_string())
    });
    let Some(value) = value else {
        return Ok(vec!["缺少 [Network] Model".to_string()]);
    };
    // ASI 允许游戏内换网络，所以只要基础模型已装就算有效；Experimental 需要安装时
    // 由 delta 生成的权重，本安装器不生成。
    let allowed: &[&str] = match models {
        ModelSelection::Original => &["CC_SILVER_AARDWOLD", "CC_SILVER_AARDWOLD_INT4_MIXED"],
        ModelSelection::Small => &["CC_SMALL_FP8"],
        ModelSelection::Both => &[
            "CC_SMALL_FP8",
            "CC_SILVER_AARDWOLD",
            "CC_SILVER_AARDWOLD_INT4_MIXED",
        ],
    };
    let matched = allowed
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate));
    Ok(if matched {
        Vec::new()
    } else {
        vec![format!("[Network] Model={value}，与已安装模型不匹配")]
    })
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
        assert!(check_fixed_profile(&path, None).unwrap().is_empty());
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
        apply_fixed_profile(&path, false, true, None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Enabled=false"));
        assert!(text.contains("InterpolationCount=3"));
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn model_selection_preserves_only_available_choices() {
        let dir = test_dir("models");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("OptiScaler.ini");
        for (models, existing, preserve, expected) in [
            (ModelSelection::Original, "1", true, "0"),
            (ModelSelection::Small, "0", true, "1"),
            (ModelSelection::Both, "0", true, "0"),
            (ModelSelection::Both, "1", true, "1"),
            (ModelSelection::Both, "invalid", true, "1"),
            (ModelSelection::Both, "0", false, "1"),
        ] {
            fs::write(
                &path,
                format!("[DlssNr]\nMochizukiNetworkModel={existing}\n"),
            )
            .unwrap();
            apply_fixed_profile(&path, false, preserve, Some(models)).unwrap();
            assert!(
                fs::read_to_string(&path)
                    .unwrap()
                    .contains(&format!("MochizukiNetworkModel={expected}"))
            );
            assert!(check_fixed_profile(&path, Some(models)).unwrap().is_empty());
        }
        apply_fixed_profile(&path, true, true, None).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("MochizukiNetworkModel=1")
        );
        assert!(
            !check_fixed_profile(&path, Some(ModelSelection::Original))
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asi_profile_copies_renodx_template_and_sets_model() {
        let dir = test_dir("asi-fresh");
        let release = dir.join("release");
        fs::create_dir_all(&release).unwrap();
        fs::write(
            release.join("dlssnr-amd-optiscaler.ini"),
            "[Network]\nModel = CC_SMALL_FP8\nRenodx=false\n",
        )
        .unwrap();
        fs::write(
            release.join("dlssnr-amd-optiscaler-renodx.ini"),
            "[Network]\nModel = CC_SMALL_FP8\nRenodx=true\n",
        )
        .unwrap();
        apply_asi_profile(&dir, &release, ModelSelection::Original).unwrap();
        let text = fs::read_to_string(dir.join("dlssnr-amd.ini")).unwrap();
        assert!(text.contains("Renodx=true"));
        assert!(text.contains("Model=CC_SILVER_AARDWOLD"));
        assert!(!text.starts_with('\u{feff}'));
        assert_eq!(
            fs::read(dir.join("dlssnr-amd/dlssnr-amd.default.ini")).unwrap(),
            text.as_bytes()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asi_profile_requires_template() {
        let dir = test_dir("asi-template");
        let release = dir.join("release");
        fs::create_dir_all(&release).unwrap();
        assert_eq!(
            apply_asi_profile(&dir, &release, ModelSelection::Original).unwrap_err(),
            "主组件缺少 dlssnr-amd.ini 模板"
        );
        assert!(!dir.join("dlssnr-amd.ini").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn asi_profile_keeps_user_content_and_switches_model() {
        let dir = test_dir("asi-switch");
        let release = dir.join("release");
        fs::create_dir_all(&release).unwrap();
        let path = dir.join("dlssnr-amd.ini");
        fs::write(
            &path,
            "; 用户注释\n[Network]\nModel=CC_SMALL_FP8\nTimeout=30\n\n[Quality]\nLevel=3\n",
        )
        .unwrap();
        apply_asi_profile(&dir, &release, ModelSelection::Small).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("Model=CC_SMALL_FP8")
        );
        apply_asi_profile(&dir, &release, ModelSelection::Original).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("Model=CC_SILVER_AARDWOLD"));
        for kept in ["; 用户注释", "Timeout=30", "Level=3"] {
            assert!(text.contains(kept), "missing {kept}");
        }
        apply_asi_profile(&dir, &release, ModelSelection::Both).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("Model=CC_SILVER_AARDWOLD")
        );
        assert_eq!(
            fs::read(dir.join("dlssnr-amd/dlssnr-amd.default.ini")).unwrap(),
            fs::read(&path).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn check_asi_profile_reports_missing_file_and_mismatched_model() {
        let dir = test_dir("asi-check");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            check_asi_profile(&dir, ModelSelection::Original).unwrap(),
            vec!["缺少 dlssnr-amd.ini".to_string()]
        );
        let path = dir.join("dlssnr-amd.ini");
        fs::write(&path, "[Network]\nModel=CC_SMALL_FP8\n").unwrap();
        let issues = check_asi_profile(&dir, ModelSelection::Original).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("CC_SMALL_FP8"));
        assert!(
            check_asi_profile(&dir, ModelSelection::Small)
                .unwrap()
                .is_empty()
        );
        assert!(
            check_asi_profile(&dir, ModelSelection::Both)
                .unwrap()
                .is_empty()
        );
        fs::write(&path, "[Network]\nModel=CC_OTHER\n").unwrap();
        let issues = check_asi_profile(&dir, ModelSelection::Both).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("CC_OTHER"));
        fs::write(&path, "[Network]\nTimeout=1\n").unwrap();
        assert!(
            !check_asi_profile(&dir, ModelSelection::Original)
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("optinr-ini-{name}-{}-{nonce}", std::process::id()))
    }
}
