use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "OptiScaler NR Installer";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const UPSTREAM_REPO: &str = "TheAutomatic/dlss-5-amd-project";
pub const FIXED_PROFILE: &str = "OptiScaler NR + Mochizuki 后端 + Intel XeFG";
pub const DEFAULT_RELEASE_REPO: &str = "DestinySail/optiscaler-nr-installer";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub cache: PathBuf,
    pub staging: PathBuf,
    pub legacy_backups: PathBuf,
    pub games: PathBuf,
    pub updates: PathBuf,
    pub settings: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self, String> {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "无法读取 LOCALAPPDATA".to_string())?;
        let root = local.join("OptiScalerNR");
        let paths = Self {
            cache: root.join("cache"),
            staging: root.join("staging"),
            legacy_backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            root,
        };
        for dir in [&paths.cache, &paths.staging, &paths.games, &paths.updates] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
        }
        Ok(paths)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtrasManifest {
    pub version: String,
    pub base_url: String,
    pub files: Vec<ExtrasFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtrasFile {
    pub path: String,
    pub key: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelSelection {
    #[default]
    Original,
    Small,
    Both,
}

impl ModelSelection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "310.8.0 原模型（FP8 + INT4）",
            Self::Small => "310.9.16 新模型（Small）",
            Self::Both => "两个模型都安装",
        }
    }

    pub fn includes(self, path: &str) -> bool {
        match path {
            "MFGUnlocker.asi" => true,
            "dlssnr-amd/dlssnr.bin" | "dlssnr-amd/dlssnr-int4.bin" => self != Self::Small,
            "dlssnr-amd/dlssnr-small.bin" => self != Self::Original,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub models: ModelSelection,
    #[serde(default)]
    pub release_repo: String,
    #[serde(default)]
    pub local_upstream_source: Option<PathBuf>,
    #[serde(default)]
    pub local_extras_source: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            models: ModelSelection::default(),
            release_repo: option_env!("NR_RELEASE_REPO")
                .unwrap_or(DEFAULT_RELEASE_REPO)
                .to_string(),
            local_upstream_source: None,
            local_extras_source: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamAsset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamRelease {
    pub release_id: u64,
    pub tag: String,
    pub published_at: String,
    pub asset: UpstreamAsset,
    pub checked_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_vram: u64,
    pub luid: u64,
    pub software: bool,
    pub mochizuki_class: GpuClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuClass {
    Rdna4,
    OtherAmd,
    Other,
    Software,
}

impl GpuClass {
    pub fn supports_fixed_profile(self) -> bool {
        self == Self::Rdna4
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameInfo {
    pub launcher: String,
    pub name: String,
    pub exe: PathBuf,
    pub game_dir: PathBuf,
    pub architecture: Option<u16>,
    pub imports: Vec<String>,
    pub compatibility: Compatibility,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Compatibility {
    Known,
    Candidate,
    Manual,
}

impl Compatibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Known => "已收录",
            Self::Candidate => "候选，需游戏内验证",
            Self::Manual => "手动选择",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedFile {
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    #[serde(default)]
    pub models: ModelSelection,
    pub schema: u32,
    pub game_name: String,
    pub exe: PathBuf,
    pub installed_at_ms: u64,
    #[serde(default = "default_true")]
    pub complete: bool,
    pub upstream_tag: String,
    pub extras_version: Option<String>,
    pub profile: String,
    pub proxy_file: String,
    pub files: Vec<ManagedFile>,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_install_record_defaults_to_complete() {
        let record: InstallRecord = serde_json::from_str(
            r#"{
                "schema": 2,
                "game_name": "Test",
                "exe": "Test.exe",
                "installed_at_ms": 1,
                "upstream_tag": "1.12.2",
                "extras_version": "extras-v1",
                "profile": "test",
                "proxy_file": "dxgi.dll",
                "files": []
            }"#,
        )
        .unwrap();
        assert!(record.complete);
        assert_eq!(record.models, ModelSelection::Original);
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Check,
    Repair,
    Update,
    Uninstall,
    DisableFrameGen,
    InstallReframework,
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Check => "检查",
            Self::Repair => "修复",
            Self::Update => "更新",
            Self::Uninstall => "卸载",
            Self::DisableFrameGen => "关闭插帧",
            Self::InstallReframework => "安装 REFramework",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub stage: String,
    pub detail: String,
    pub fraction: Option<f32>,
}

impl Progress {
    pub fn new(stage: impl Into<String>, detail: impl Into<String>, fraction: Option<f32>) -> Self {
        Self {
            stage: stage.into(),
            detail: detail.into(),
            fraction,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaintenanceReport {
    pub title: String,
    pub lines: Vec<String>,
}
