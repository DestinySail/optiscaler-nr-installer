use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "OptiScaler NR Installer";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const UPSTREAM_REPO: &str = "TheAutomatic/dlss-5-amd-project";
pub const FIXED_PROFILE: &str = "政宗 SAMA OptiScaler NR + Mochizuki + Intel XeFG";
pub const DEFAULT_RELEASE_REPO: &str = "DestinySail/optiscaler-nr-installer";
pub const DEFAULT_EXTRAS_VERSION: &str = "extras-v1";
pub const DEFAULT_EXTRAS_URL: &str = "https://github.com/DestinySail/optiscaler-nr-installer/releases/download/extras-v1/extras-v1.zip";
pub const DEFAULT_EXTRAS_SHA256: &str =
    "ec993a9a75772c2a178d167be43a1760218001a68e6a367f1caa93be9e400533";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub cache: PathBuf,
    pub staging: PathBuf,
    pub backups: PathBuf,
    pub games: PathBuf,
    pub updates: PathBuf,
    pub settings: PathBuf,
    pub upstream_check: PathBuf,
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
            backups: root.join("backups"),
            games: root.join("games"),
            updates: root.join("updates"),
            settings: root.join("settings.json"),
            upstream_check: root.join("catalog").join("upstream.json"),
            root,
        };
        for dir in [
            &paths.cache,
            &paths.staging,
            &paths.backups,
            &paths.games,
            &paths.updates,
            paths.upstream_check.parent().expect("catalog has a parent"),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
        }
        Ok(paths)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtrasSpec {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub release_repo: String,
    #[serde(default)]
    pub extras: Option<ExtrasSpec>,
    #[serde(default)]
    pub local_upstream_source: Option<PathBuf>,
    #[serde(default)]
    pub local_extras_source: Option<PathBuf>,
    #[serde(default)]
    pub auto_apply_app_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let extras = Some(ExtrasSpec {
            version: option_env!("NR_EXTRAS_VERSION")
                .unwrap_or(DEFAULT_EXTRAS_VERSION)
                .to_string(),
            url: option_env!("NR_EXTRAS_URL")
                .unwrap_or(DEFAULT_EXTRAS_URL)
                .to_string(),
            sha256: option_env!("NR_EXTRAS_SHA256")
                .unwrap_or(DEFAULT_EXTRAS_SHA256)
                .to_string(),
        });
        Self {
            release_repo: option_env!("NR_RELEASE_REPO")
                .unwrap_or(DEFAULT_RELEASE_REPO)
                .to_string(),
            extras,
            local_upstream_source: None,
            local_extras_source: None,
            auto_apply_app_update: false,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamCheckCache {
    pub latest: UpstreamRelease,
    pub etag: Option<String>,
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
    pub schema: u32,
    pub game_name: String,
    pub exe: PathBuf,
    pub installed_at_ms: u64,
    pub upstream_tag: String,
    pub extras_version: Option<String>,
    pub profile: String,
    pub proxy_file: String,
    #[serde(default = "default_true")]
    pub frame_gen_enabled: bool,
    pub files: Vec<ManagedFile>,
    pub baseline_backup: PathBuf,
    pub previous_snapshot: PathBuf,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub enum Action {
    Check,
    Repair,
    Update,
    Uninstall,
    DisableFrameGen,
    Rollback,
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Check => "检查",
            Self::Repair => "修复",
            Self::Update => "更新",
            Self::Uninstall => "卸载",
            Self::DisableFrameGen => "关闭插帧",
            Self::Rollback => "回滚",
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
