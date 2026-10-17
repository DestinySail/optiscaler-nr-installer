use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
};

use crate::model::{Compatibility, GameInfo};
use crate::pe;

const KNOWN_EXES: &[(&str, Compatibility)] = &[
    ("Client-Win64-Shipping.exe", Compatibility::Known),
    ("Cyberpunk2077.exe", Compatibility::Candidate),
    ("Palworld-Win64-Shipping.exe", Compatibility::Candidate),
    ("HorizonForbiddenWest.exe", Compatibility::Candidate),
    ("ForzaHorizon6.exe", Compatibility::Candidate),
    ("SHf.exe", Compatibility::Candidate),
];

const BAD_EXE_PARTS: &[&str] = &[
    "crash",
    "report",
    "unins",
    "setup",
    "launcher",
    "redist",
    "vcredist",
    "benchmark",
    "editor",
    "server",
    "tool",
    "helper",
    "anticheat",
    "easyanticheat",
    "battleye",
    "installer",
    "wallpaper",
    "browser",
];

const MIN_EXE_BYTES: u64 = 64 * 1024;

const NON_GAME_STEAM_NAMES: &[&str] = &[
    "steamworks common redistributables",
    "steamvr",
    "steam controller configs",
    "wallpaper engine",
];

pub fn discover_games() -> Vec<GameInfo> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for root in steam_library_roots() {
        scan_library(&root, "Steam", &mut seen, &mut found);
    }
    for (name, root) in epic_games() {
        if let Some(game) = scan_game_dir(&name, "Epic", &root) {
            push_unique(game, &mut seen, &mut found);
        }
    }
    for (name, root) in registry_games() {
        if let Some(game) = scan_game_dir(&name, "Windows", &root) {
            push_unique(game, &mut seen, &mut found);
        }
    }
    for root in kuro_roots() {
        if let Some(game) = scan_game_dir("鸣潮", "Kuro", &root) {
            push_unique(game, &mut seen, &mut found);
        }
    }
    found.sort_by_key(|game| game.name.to_ascii_lowercase());
    found
}

pub fn default_picker_directory() -> Option<PathBuf> {
    steam_library_roots()
        .into_iter()
        .map(|root| root.join("steamapps").join("common"))
        .find(|common| common.is_dir())
        .or_else(|| kuro_roots().into_iter().next())
}

pub fn inspect_manual(exe: PathBuf) -> Result<GameInfo, String> {
    if !exe.is_file() {
        return Err("选择的 EXE 不存在".to_string());
    }
    let info = pe::inspect(&exe).map_err(|e| format!("无法识别所选 EXE: {e}"))?;
    if !info.is_64_bit {
        return Err("所选游戏不是 64 位程序，固定方案不支持".to_string());
    }
    let game_dir = exe
        .parent()
        .ok_or_else(|| "无法取得游戏目录".to_string())?
        .to_path_buf();
    let name = exe
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("手动选择的游戏")
        .to_string();
    Ok(GameInfo {
        launcher: "手动".to_string(),
        name,
        exe,
        game_dir,
        architecture: Some(64),
        imports: info.imports,
        compatibility: Compatibility::Manual,
    })
}

fn steam_library_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = steam_path() {
        push_library_root(&mut roots, path);
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = std::env::var_os(var) {
            push_library_root(&mut roots, PathBuf::from(base).join("Steam"));
        }
    }
    let mut expanded = Vec::new();
    for root in roots {
        expanded.push(root.clone());
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = fs::read_to_string(vdf) {
            for line in text.lines() {
                let trimmed = line.trim();
                if !trimmed.to_ascii_lowercase().starts_with("\"path\"") {
                    continue;
                }
                if let Some(value) = quoted_values(trimmed).into_iter().nth(1) {
                    let path = PathBuf::from(value.replace("\\\\", "\\"));
                    if path.is_dir() {
                        expanded.push(path);
                    }
                }
            }
        }
    }
    expanded.sort();
    expanded.dedup();
    expanded
}

fn steam_path() -> Option<PathBuf> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .ok()?
        .get_value::<String, _>("SteamPath")
        .ok()
        .map(PathBuf::from)
}

fn push_library_root(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if path.join("steamapps").is_dir() {
        roots.push(path);
    }
}

fn scan_library(
    steam_root: &Path,
    launcher: &str,
    seen: &mut HashSet<String>,
    found: &mut Vec<GameInfo>,
) {
    let steamapps = steam_root.join("steamapps");
    let common = steamapps.join("common");
    let Ok(entries) = fs::read_dir(&steamapps) else {
        return;
    };
    let mut manifests = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("appmanifest_") && name.ends_with(".acf")
        })
        .take(500)
        .collect::<Vec<_>>();

    if manifests.is_empty() {
        let Ok(entries) = fs::read_dir(common) else {
            return;
        };
        manifests = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .take(500)
            .collect();
        for entry in manifests {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(game) = scan_game_dir(&name, launcher, &entry.path()) {
                push_unique(game, seen, found);
            }
        }
        return;
    }

    for manifest in manifests {
        let Ok(text) = fs::read_to_string(manifest.path()) else {
            continue;
        };
        let Some(name) = acf_value(&text, "name") else {
            continue;
        };
        if NON_GAME_STEAM_NAMES
            .iter()
            .any(|tool| name.eq_ignore_ascii_case(tool))
        {
            continue;
        }
        if let Some(flags) = acf_value(&text, "StateFlags")
            && let Ok(flags) = flags.parse::<u32>()
            && flags & 4 == 0
        {
            continue;
        }
        let Some(install_dir) = acf_value(&text, "installdir") else {
            continue;
        };
        if !is_single_folder_name(&install_dir) {
            continue;
        }
        let root = common.join(install_dir);
        if root.is_dir()
            && let Some(game) = scan_game_dir(&name, launcher, &root)
        {
            push_unique(game, seen, found);
        }
    }
}

fn acf_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let values = quoted_values(line);
        (values.len() >= 2 && values[0].eq_ignore_ascii_case(key))
            .then(|| values[1].trim().to_string())
    })
}

fn is_single_folder_name(value: &str) -> bool {
    !value.trim_end_matches(['.', ' ']).is_empty()
        && !Path::new(value).is_absolute()
        && !value.contains(['\\', '/', ':'])
}

fn epic_games() -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    let manifest = std::env::var_os("ProgramData").map(PathBuf::from).map(|p| {
        p.join("Epic")
            .join("UnrealEngineLauncher")
            .join("LauncherInstalled.dat")
    });
    if let Some(path) = manifest
        && let Ok(text) = fs::read_to_string(path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        for app in json
            .get("InstallationList")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let app_name = app
                .get("AppName")
                .and_then(|v| v.as_str())
                .unwrap_or("Epic 游戏");
            if let Some(location) = app.get("InstallLocation").and_then(|v| v.as_str()) {
                result.push((friendly_epic_name(app_name), PathBuf::from(location)));
            }
        }
    }
    result
}

fn friendly_epic_name(app_name: &str) -> String {
    app_name.replace('-', " ").trim().to_string()
}

fn kuro_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Wuthering Waves"));
    }
    for drive in ["C:", "D:", "E:", "F:"] {
        roots.push(PathBuf::from(drive).join("Wuthering Waves"));
        roots.push(PathBuf::from(drive).join("Games").join("Wuthering Waves"));
    }
    roots.into_iter().filter(|path| path.is_dir()).collect()
}

fn registry_games() -> Vec<(String, PathBuf)> {
    let mut games = Vec::new();
    for flags in [KEY_READ | KEY_WOW64_64KEY, KEY_READ | KEY_WOW64_32KEY] {
        for hive in [
            RegKey::predef(HKEY_CURRENT_USER),
            RegKey::predef(HKEY_LOCAL_MACHINE),
        ] {
            let Ok(uninstall) = hive.open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
                flags,
            ) else {
                continue;
            };
            for name in uninstall.enum_keys().flatten() {
                let Ok(app) = uninstall.open_subkey_with_flags(&name, flags) else {
                    continue;
                };
                let display_name = app
                    .get_value::<String, _>("DisplayName")
                    .unwrap_or_default();
                if !wanted_registry_game(&display_name) {
                    continue;
                }
                let install_location = app
                    .get_value::<String, _>("InstallLocation")
                    .unwrap_or_default();
                let uninstall_string = app
                    .get_value::<String, _>("UninstallString")
                    .unwrap_or_default();
                let display_icon = app
                    .get_value::<String, _>("DisplayIcon")
                    .unwrap_or_default();
                let root = [install_location, uninstall_string, display_icon]
                    .into_iter()
                    .filter_map(|value| path_from_registry_value(&value))
                    .find(|path| path.is_dir());
                if let Some(root) = root {
                    games.push((display_name, root));
                }
            }
        }
    }
    games
}

fn wanted_registry_game(name: &str) -> bool {
    [
        "Wuthering Waves",
        "鸣潮",
        "Cyberpunk 2077",
        "Palworld",
        "Horizon Forbidden West",
        "Forza Horizon 6",
        "Silent Hill",
    ]
    .iter()
    .any(|needle| {
        name.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }) && !name.contains("助手")
}

fn path_from_registry_value(value: &str) -> Option<PathBuf> {
    let value = value.trim().trim_matches('"');
    if value.is_empty() {
        return None;
    }
    let expanded = value
        .split(',')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_matches('"');
    let path = PathBuf::from(expanded);
    if path.is_file() {
        path.parent().map(Path::to_path_buf)
    } else {
        Some(path)
    }
}

fn scan_game_dir(name: &str, launcher: &str, root: &Path) -> Option<GameInfo> {
    let mut candidates = Vec::new();
    collect_exes(root, 0, name, &mut candidates);
    candidates.sort_by_key(|(score, _, _)| *score);
    candidates.pop().map(|(_, exe, info)| {
        let wuthering = root
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("wuthering waves")
            || name.contains("鸣潮");
        let compatibility = if wuthering {
            Compatibility::Known
        } else {
            Compatibility::Candidate
        };
        GameInfo {
            launcher: launcher.to_string(),
            name: display_name(name, &exe),
            game_dir: exe.parent().unwrap_or(root).to_path_buf(),
            exe,
            architecture: info.is_64_bit.then_some(64),
            imports: info.imports,
            compatibility,
        }
    })
}

fn collect_exes(
    root: &Path,
    depth: usize,
    game_name: &str,
    out: &mut Vec<(u64, PathBuf, pe::PeInfo)>,
) {
    if depth > 6 || out.len() > 200 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten().take(1000) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !matches!(
                name.as_str(),
                "_commonredist"
                    | "redist"
                    | "redistributables"
                    | "support"
                    | "tools"
                    | "editor"
                    | "engine"
            ) {
                collect_exes(&path, depth + 1, game_name, out);
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        if !is_game_exe_name(name) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.len() < MIN_EXE_BYTES {
            continue;
        }
        let Ok(info) = pe::inspect(&path) else {
            continue;
        };
        if !info.is_64_bit {
            continue;
        }
        let mut score = (meta.len() / (1024 * 1024)).min(500);
        let exe_stem = path.file_stem().and_then(|v| v.to_str()).unwrap_or("");
        let normalized_exe = normalize_name(exe_stem);
        let normalized_game = normalize_name(game_name);
        if normalized_exe.len() >= 3
            && (normalized_game.starts_with(&normalized_exe)
                || normalized_exe.starts_with(&normalized_game))
        {
            score += 6_000;
        }
        if KNOWN_EXES
            .iter()
            .any(|(known, _)| lower.eq_ignore_ascii_case(&known.to_ascii_lowercase()))
        {
            score += 10_000;
        }
        if lower.contains("shipping") || lower.contains("win64") {
            score += 500;
        }
        if info.imports.iter().any(|import| {
            matches!(
                import.to_ascii_lowercase().as_str(),
                "dxgi.dll" | "d3d12.dll"
            )
        }) {
            score += 1_000;
        } else if info
            .imports
            .iter()
            .any(|import| import.eq_ignore_ascii_case("d3d11.dll"))
        {
            score += 300;
        }
        out.push((score, path, info));
    }
}

fn is_game_exe_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !BAD_EXE_PARTS.iter().any(|part| lower.contains(part))
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn display_name(folder: &str, exe: &Path) -> String {
    if folder.to_ascii_lowercase().contains("wuthering") {
        return "鸣潮".to_string();
    }
    if let Some((known, _)) = KNOWN_EXES.iter().find(|(known, _)| {
        exe.file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|actual| actual.eq_ignore_ascii_case(known))
    }) {
        if *known == "Cyberpunk2077.exe" {
            return "赛博朋克 2077".to_string();
        }
        if *known == "Palworld-Win64-Shipping.exe" {
            return "幻兽帕鲁".to_string();
        }
    }
    folder.to_string()
}

fn push_unique(game: GameInfo, seen: &mut HashSet<String>, found: &mut Vec<GameInfo>) {
    let key = game
        .exe
        .canonicalize()
        .unwrap_or_else(|_| game.exe.clone())
        .to_string_lossy()
        .to_ascii_lowercase();
    if seen.insert(key) {
        found.push(game);
    }
}

fn quoted_values(line: &str) -> Vec<String> {
    line.split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vdf_quoted_values() {
        assert_eq!(
            quoted_values("\"path\" \"D:\\\\SteamLibrary\""),
            vec!["path", "D:\\\\SteamLibrary"]
        );
    }

    #[test]
    fn parses_steam_manifest_fields() {
        let manifest = "\"AppState\"\n{\n    \"appid\" \"3357650\"\n    \"name\" \"PRAGMATA\"\n    \"installdir\" \"PRAGMATA\"\n}";
        assert_eq!(acf_value(manifest, "name").as_deref(), Some("PRAGMATA"));
        assert_eq!(
            acf_value(manifest, "installdir").as_deref(),
            Some("PRAGMATA")
        );
    }

    #[test]
    fn rejects_helpers_but_keeps_small_game_exes() {
        assert!(is_game_exe_name("WarmSnow.exe"));
        assert!(is_game_exe_name("EDF6.exe"));
        assert!(!is_game_exe_name("UnityCrashHandler64.exe"));
        assert!(!is_game_exe_name("ZFGameBrowser.exe"));
    }
}
