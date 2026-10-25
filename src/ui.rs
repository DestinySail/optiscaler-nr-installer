use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui;

use crate::assets;
use crate::games;
use crate::gpu;
use crate::install;
use crate::model::{
    APP_NAME, APP_VERSION, Action, AppPaths, FIXED_PROFILE, GameInfo, GpuInfo, MaintenanceReport,
    ModelSelection, Progress, Settings, UpstreamRelease,
};
use crate::self_update::{self, AppUpdateInfo};
use crate::updates;
use crate::util::{file_contains_marker, read_json, write_json_atomic};

const BG: egui::Color32 = egui::Color32::from_rgb(12, 18, 30);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(12, 17, 29);
const CARD: egui::Color32 = egui::Color32::from_rgb(17, 25, 39);
const CARD_ALT: egui::Color32 = egui::Color32::from_rgb(20, 30, 44);
const LINE: egui::Color32 = egui::Color32::from_rgb(35, 48, 71);
const TEXT: egui::Color32 = egui::Color32::from_rgb(235, 243, 255);
const TEXT2: egui::Color32 = egui::Color32::from_rgb(143, 156, 177);
const MUTED: egui::Color32 = egui::Color32::from_rgb(100, 117, 141);
const CYAN: egui::Color32 = egui::Color32::from_rgb(119, 227, 234);
const CYAN_DIM: egui::Color32 = egui::Color32::from_rgb(78, 143, 166);
const BLUE: egui::Color32 = egui::Color32::from_rgb(93, 157, 252);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(148, 136, 255);
const GREEN: egui::Color32 = egui::Color32::from_rgb(114, 227, 176);
const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 196, 126);
const RED: egui::Color32 = egui::Color32::from_rgb(255, 135, 147);
const GAME_ROW_GAP: f32 = 6.0;
const CONTENT_INSET: f32 = 12.0;
const ACTION_HEIGHT: f32 = 42.0;
const DIALOG_WIDTH: f32 = 480.0;
const DIALOG_PADDING: i8 = 24;

#[derive(Debug)]
enum Command {
    Refresh,
    CheckUpdates(Settings, bool),
    Install {
        game: GameInfo,
        settings: Settings,
        upstream: Option<UpstreamRelease>,
        main_package_only: bool,
    },
    Maintain {
        game: GameInfo,
        action: Action,
        settings: Settings,
        upstream: Option<UpstreamRelease>,
    },
    ApplyAppUpdate(AppUpdateInfo),
}

#[derive(Debug)]
enum Event {
    Gpus(Result<Vec<GpuInfo>, String>),
    Games(Vec<GameInfo>),
    Checked {
        upstream: Result<Box<UpstreamRelease>, String>,
        app_update: Result<Option<AppUpdateInfo>, String>,
        warning: Option<String>,
    },
    Progress(Progress),
    Installed(Result<MaintenanceReport, String>),
    Maintained {
        action: Action,
        result: Result<MaintenanceReport, String>,
    },
    AppUpdateProgress(Progress),
    AppUpdateApplied(Result<(), String>),
    Status(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Select,
    Installing,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Install,
    Maintain,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppUpdatePhase {
    Prompt,
    Downloading,
    Ready,
    Failed,
}

#[derive(Clone, Copy)]
enum NavIcon {
    Home,
    Layers,
    Sliders,
}

pub struct App {
    paths: Result<AppPaths, String>,
    settings: Settings,
    draft: Settings,
    gpus: Vec<GpuInfo>,
    games: Vec<GameInfo>,
    scanning_games: bool,
    install_statuses: HashMap<String, String>,
    selected_game: Option<usize>,
    upstream: Option<UpstreamRelease>,
    checking_updates: bool,
    upstream_error: Option<String>,
    last_update_check: Option<String>,
    app_update: Option<AppUpdateInfo>,
    screen: Screen,
    page: Page,
    progress: Progress,
    report: Option<MaintenanceReport>,
    error: Option<String>,
    status: String,
    maintenance_busy: bool,
    pending_maintenance: Option<(GameInfo, Action)>,
    app_update_busy: bool,
    app_update_phase: Option<AppUpdatePhase>,
    app_update_progress: Progress,
    app_update_error: Option<String>,
    update_notified_version: Option<String>,
    last_maintenance: Option<MaintenanceReport>,
    sender: Sender<Command>,
    receiver: Receiver<Event>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Result<AppPaths, String>) -> Self {
        install_fonts(&cc.egui_ctx);
        apply_style(&cc.egui_ctx);
        let settings = paths
            .as_ref()
            .ok()
            .and_then(|paths| read_json::<Settings>(&paths.settings).ok())
            .unwrap_or_default();
        let (sender, receiver) = spawn_worker(paths.clone());
        let upstream = paths
            .as_ref()
            .ok()
            .and_then(|paths| updates::load_cached_upstream(paths, &settings));
        let mut app = Self {
            paths,
            settings: settings.clone(),
            draft: settings,
            gpus: Vec::new(),
            games: Vec::new(),
            scanning_games: false,
            install_statuses: HashMap::new(),
            selected_game: None,
            upstream,
            checking_updates: false,
            upstream_error: None,
            last_update_check: None,
            app_update: None,
            screen: Screen::Select,
            page: Page::Install,
            progress: Progress::new("准备", "正在检测设备", None),
            report: None,
            error: None,
            status: "正在检测显卡和游戏…".to_string(),
            maintenance_busy: false,
            pending_maintenance: None,
            app_update_busy: false,
            app_update_phase: None,
            app_update_progress: Progress::new("安装器更新", "准备下载", None),
            app_update_error: None,
            update_notified_version: None,
            last_maintenance: None,
            sender,
            receiver,
        };
        app.refresh_games();
        app.check_updates(true);
        app
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Gpus(result) => match result {
                    Ok(gpus) => self.gpus = gpus,
                    Err(error) => self.error = Some(error),
                },
                Event::Games(games) => {
                    self.scanning_games = false;
                    let selected = self.selected_game().map(|game| game.exe.clone());
                    self.games = games;
                    self.selected_game = selected
                        .and_then(|exe| self.games.iter().position(|game| game.exe == exe))
                        .or_else(|| (!self.games.is_empty()).then_some(0));
                    self.refresh_installed_tags();
                    self.status = format!("已发现 {} 个候选游戏", self.games.len());
                }
                Event::Checked {
                    upstream,
                    app_update,
                    warning,
                } => {
                    self.checking_updates = false;
                    let installer_status = match &app_update {
                        Ok(Some(update)) => format!("安装器：可更新至 {}", update.version),
                        Ok(None) if self.settings.release_repo.trim().is_empty() => {
                            "安装器：未配置更新源".into()
                        }
                        Ok(None) => "安装器：已是最新版本".into(),
                        Err(_) => "安装器：更新检查失败".into(),
                    };
                    let component_status = match &upstream {
                        Ok(release) => format!("组件主包：最新 {}", release.tag),
                        Err(error) => format!("组件主包：检查失败（{error}）"),
                    };
                    match upstream {
                        Ok(release) => {
                            self.upstream_error = None;
                            self.status =
                                format!("主包组件：最新 {}；{installer_status}", release.tag);
                            self.upstream = Some(*release);
                        }
                        Err(error) => {
                            self.status = format!(
                                "主包组件：更新检查失败；{installer_status}。主包组件详情：{error}"
                            );
                            self.upstream_error = Some(error);
                            if self.upstream.is_none() {
                                self.upstream = self.paths.as_ref().ok().and_then(|paths| {
                                    updates::load_cached_upstream(paths, &self.settings)
                                });
                            }
                        }
                    }
                    match app_update {
                        Ok(update) => {
                            self.app_update = update;
                            if let Some(update) = &self.app_update
                                && self.update_notified_version.as_deref()
                                    != Some(update.version.as_str())
                            {
                                self.update_notified_version = Some(update.version.clone());
                                self.app_update_phase = Some(AppUpdatePhase::Prompt);
                                self.app_update_error = None;
                            }
                        }
                        Err(error) => {
                            self.status.push_str(&format!("；安装器详情：{error}"));
                            self.last_maintenance = Some(MaintenanceReport {
                                title: "安装器更新检查失败".to_string(),
                                lines: vec![error],
                            })
                        }
                    }
                    if let Some(warning) = warning {
                        self.upstream_error = Some(warning.clone());
                        self.status = format!(
                            "主包组件与安装器均使用已验签缓存（非实时结果）；更新源暂不可用：{warning}"
                        );
                    }
                    self.last_update_check = Some(format!(
                        "{component_status}；{installer_status}{}",
                        if self.upstream_error.is_some() {
                            "（使用已验签缓存）"
                        } else {
                            ""
                        }
                    ));
                }
                Event::Progress(progress) => self.progress = progress,
                Event::Installed(result) => match result {
                    Ok(report) => {
                        self.report = Some(report);
                        self.screen = Screen::Done;
                        self.refresh_installed_tags();
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.screen = Screen::Select;
                    }
                },
                Event::Maintained { action, result } => {
                    self.maintenance_busy = false;
                    self.last_maintenance = Some(match result {
                        Ok(report) => report,
                        Err(error) => MaintenanceReport {
                            title: format!("{}失败", action.label()),
                            lines: vec![error],
                        },
                    });
                    self.refresh_installed_tags();
                }
                Event::AppUpdateProgress(progress) => {
                    self.app_update_progress = progress;
                }
                Event::AppUpdateApplied(result) => {
                    self.app_update_busy = false;
                    match result {
                        Ok(()) => {
                            self.app_update_phase = Some(AppUpdatePhase::Ready);
                            self.app_update_error = None;
                        }
                        Err(error) => {
                            self.app_update_phase = Some(AppUpdatePhase::Failed);
                            self.app_update_error = Some(error);
                        }
                    }
                }
                Event::Status(status) => self.status = status,
            }
        }
    }

    fn selected_game(&self) -> Option<&GameInfo> {
        self.selected_game.and_then(|index| self.games.get(index))
    }

    fn refresh_installed_tags(&mut self) {
        self.install_statuses.clear();
        let Some(paths) = self.paths.as_ref().ok() else {
            return;
        };
        for game in &self.games {
            let status = if let Some(status) = install::install_status(paths, game) {
                status
            } else if file_contains_marker(&game.game_dir.join("dxgi.dll"), "OptiScaler")
                .unwrap_or(false)
            {
                "外部安装".to_string()
            } else {
                "未安装".to_string()
            };
            self.install_statuses
                .insert(installed_key(&game.exe), status);
        }
        self.sort_games_by_install();
    }

    fn sort_games_by_install(&mut self) {
        let selected = self.selected_game().map(|game| game.exe.clone());
        let statuses = &self.install_statuses;
        self.games.sort_by_cached_key(|game| {
            !statuses
                .get(&installed_key(&game.exe))
                .is_some_and(|status| is_managed_status(status))
        });
        self.selected_game =
            selected.and_then(|exe| self.games.iter().position(|game| game.exe == exe));
    }

    fn install_status(&self, game: &GameInfo) -> String {
        self.install_statuses
            .get(&installed_key(&game.exe))
            .cloned()
            .unwrap_or_else(|| "未安装".to_string())
    }

    fn can_install(&self) -> bool {
        self.gpus
            .iter()
            .any(|gpu| gpu.mochizuki_class.supports_fixed_profile())
    }

    #[cfg(test)]
    fn game_has_install(&self, game: &GameInfo) -> bool {
        self.install_statuses
            .get(&installed_key(&game.exe))
            .is_some_and(|status| is_managed_status(status))
    }

    fn game_needs_update(&self, game: &GameInfo) -> bool {
        self.upstream.as_ref().is_some_and(|latest| {
            self.install_statuses
                .get(&installed_key(&game.exe))
                .and_then(|status| status.strip_prefix("已安装 "))
                .is_some_and(|installed| newer_version(&latest.tag, installed))
        })
    }

    fn check_updates(&mut self, force: bool) {
        if !self.checking_updates {
            self.checking_updates = true;
            self.status = "正在检查主包组件和安装器更新…".into();
            self.last_update_check = Some("正在检查组件主包和安装器版本…".into());
            let _ = self
                .sender
                .send(Command::CheckUpdates(self.settings.clone(), force));
        }
    }

    fn refresh_games(&mut self) {
        if self.scanning_games {
            return;
        }
        match self.sender.send(Command::Refresh) {
            Ok(()) => {
                self.scanning_games = true;
                self.status = "正在扫描游戏…".into();
            }
            Err(_) => self.error = Some("无法启动游戏扫描，请重新打开安装器。".into()),
        }
    }

    fn save_settings(&mut self) {
        let Some(paths) = self.paths.as_ref().ok() else {
            return;
        };
        match write_json_atomic(&paths.settings, &self.draft) {
            Ok(()) => {
                self.settings = self.draft.clone();
                self.status = "设置已保存".to_string();
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn choose_upstream_source(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP", &["zip"])
            .pick_file()
        {
            self.draft.local_upstream_source = Some(path);
        }
    }

    fn choose_extras_source(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.draft.local_extras_source = Some(path);
        }
    }

    fn choose_game_exe(&mut self) {
        let mut dialog = rfd::FileDialog::new().add_filter("游戏 EXE", &["exe"]);
        if let Some(game) = self.selected_game() {
            dialog = dialog.set_directory(&game.game_dir);
        } else if let Some(directory) = games::default_picker_directory() {
            dialog = dialog.set_directory(directory);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        match games::inspect_manual(path) {
            Ok(game) => {
                self.games.push(game);
                self.selected_game = Some(self.games.len() - 1);
                self.refresh_installed_tags();
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn install_selected(&mut self) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        if !self.can_install() {
            self.error = Some("当前没有检测到受支持的 RDNA4 显卡".to_string());
            return;
        }
        self.screen = Screen::Installing;
        self.error = None;
        self.progress = Progress::new("预检查", "准备固定方案", Some(0.0));
        let main_package_only = self.install_status(&game).starts_with("已安装 ")
            && self.game_needs_update(&game)
            && self
                .paths
                .as_ref()
                .ok()
                .and_then(|paths| install::load_record(paths, &game).ok())
                .is_some_and(|record| record.models == self.settings.models);
        let _ = self.sender.send(Command::Install {
            game,
            settings: self.settings.clone(),
            upstream: self.upstream.clone(),
            main_package_only,
        });
    }

    fn run_maintenance(&mut self, game: GameInfo, action: Action) {
        self.maintenance_busy = true;
        self.last_maintenance = None;
        let _ = self.sender.send(Command::Maintain {
            game,
            action,
            settings: self.settings.clone(),
            upstream: self.upstream.clone(),
        });
    }

    fn start_app_update(&mut self) {
        let Some(update) = self.app_update.clone() else {
            return;
        };
        self.app_update_busy = true;
        self.app_update_phase = Some(AppUpdatePhase::Downloading);
        self.app_update_error = None;
        self.app_update_progress = Progress::new("安装器更新", "准备下载", Some(0.0));
        let _ = self.sender.send(Command::ApplyAppUpdate(update));
    }

    fn app_update_modal(&mut self, ctx: &egui::Context) {
        let Some(update) = self.app_update.clone() else {
            return;
        };
        let Some(phase) = self.app_update_phase else {
            return;
        };
        let mut dismiss = false;
        let mut download = false;
        let mut restart = false;
        let modal = egui::Modal::new(egui::Id::unique("app-update-dialog"))
            .frame(
                egui::Frame::NONE
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(DIALOG_PADDING)),
            )
            .show(ctx, |ui| {
                ui.set_width(
                    DIALOG_WIDTH.min(ctx.content_rect().width() - 2.0 * DIALOG_PADDING as f32),
                );
                ui.label(
                    egui::RichText::new("安装器更新")
                        .font(bold(20.0))
                        .color(TEXT),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("发现新版本 {}", update.version))
                        .size(12.0)
                        .color(CYAN),
                );
                ui.add_space(6.0);
                if !update.notes.is_empty() {
                    ui.label(
                        egui::RichText::new(truncate(&update.notes, 240))
                            .size(10.5)
                            .color(TEXT2),
                    );
                }
                ui.add_space(16.0);
                match phase {
                    AppUpdatePhase::Prompt => {
                        ui.label(
                            egui::RichText::new("更新会下载并校验新安装器，完成后需要重新启动。")
                                .size(11.0)
                                .color(TEXT2),
                        );
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if primary_button(ui, "下载更新", true, 132.0) {
                                        download = true;
                                    }
                                    if secondary_button(ui, "稍后", 104.0).clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        });
                    }
                    AppUpdatePhase::Downloading => {
                        ui.add(
                            egui::ProgressBar::new(
                                self.app_update_progress.fraction.unwrap_or(0.0),
                            )
                            .show_percentage(),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new(&self.app_update_progress.detail)
                                    .size(10.5)
                                    .color(TEXT2),
                            );
                        });
                    }
                    AppUpdatePhase::Ready => {
                        ui.label(
                            egui::RichText::new("更新已下载并完成校验。重新启动后使用新版本。")
                                .size(11.0)
                                .color(GREEN),
                        );
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if primary_button(ui, "重新启动", true, 132.0) {
                                        restart = true;
                                    }
                                },
                            );
                        });
                    }
                    AppUpdatePhase::Failed => {
                        let error = self.app_update_error.as_deref().unwrap_or("安装器更新失败");
                        ui.label(egui::RichText::new(error).size(10.5).color(RED));
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if primary_button(ui, "重试下载", true, 132.0) {
                                        download = true;
                                    }
                                    if secondary_button(ui, "关闭", 104.0).clicked() {
                                        dismiss = true;
                                    }
                                },
                            );
                        });
                    }
                }
            });
        if restart {
            let exe = std::env::current_exe()
                .map_err(|error| format!("读取安装器路径失败: {error}"))
                .and_then(|exe| {
                    std::process::Command::new(exe)
                        .arg("--restart")
                        .spawn()
                        .map(|_| ())
                        .map_err(|error| format!("重新启动安装器失败: {error}"))
                });
            match exe {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(error) => {
                    self.app_update_phase = Some(AppUpdatePhase::Failed);
                    self.app_update_error = Some(error);
                }
            }
        } else if download {
            self.start_app_update();
        } else if dismiss || modal.should_close() {
            self.app_update_phase = None;
        }
    }

    fn maintenance_report_modal(&mut self, ctx: &egui::Context) {
        if self.app_update_phase.is_some() {
            return;
        }
        let Some(report) = self.last_maintenance.clone() else {
            return;
        };
        let failed = report.title.contains("失败");
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("maintenance-report-dialog"))
            .frame(
                egui::Frame::NONE
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(DIALOG_PADDING)),
            )
            .show(ctx, |ui| {
                ui.set_width(
                    DIALOG_WIDTH.min(ctx.content_rect().width() - 2.0 * DIALOG_PADDING as f32),
                );
                ui.label(
                    egui::RichText::new(&report.title)
                        .font(bold(20.0))
                        .color(if failed { RED } else { GREEN }),
                );
                ui.add_space(12.0);
                egui::ScrollArea::vertical()
                    .id_salt("maintenance-report-body")
                    .max_height((ctx.content_rect().height() - 230.0).clamp(80.0, 260.0))
                    .show(ui, |ui| {
                        for line in &report.lines {
                            ui.add(
                                egui::Label::new(egui::RichText::new(line).size(11.5).color(TEXT2))
                                    .wrap(),
                            );
                            ui.add_space(5.0);
                        }
                    });
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "关闭", true, 120.0) {
                            close = true;
                        }
                    });
                });
            });
        if close || modal.should_close() {
            self.last_maintenance = None;
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
            draw_logo(ui.painter(), rect);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("OptiScaler NR")
                        .font(bold(16.5))
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new("I N S T A L L E R")
                        .size(9.0)
                        .color(MUTED),
                );
            });
        });
        ui.add_space(16.0);
        let (divider, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().hline(
            divider.x_range(),
            divider.center().y,
            egui::Stroke::new(1.0, LINE),
        );
        ui.add_space(20.0);
        ui.label(egui::RichText::new("WORKSPACE").size(9.0).color(MUTED));
        ui.add_space(12.0);
        self.nav_item(ui, Page::Install, "安装中心", NavIcon::Home);
        self.nav_item(ui, Page::Maintain, "维护与管理", NavIcon::Layers);
        self.nav_item(ui, Page::Settings, "偏好设置", NavIcon::Sliders);

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("VERSION {APP_VERSION}"))
                        .size(8.5)
                        .color(MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("© DestinySail").size(8.5).color(MUTED));
                });
            });
            ui.add_space(14.0);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 112.0),
                egui::Sense::hover(),
            );
            let gpu = self
                .gpus
                .iter()
                .find(|gpu| gpu.mochizuki_class.supports_fixed_profile())
                .or_else(|| self.gpus.iter().find(|gpu| !gpu.software));
            let supported = self.can_install();
            let name = gpu.map(|gpu| gpu.name.as_str()).unwrap_or("正在检测…");
            let detail = if supported {
                "AMD RDNA4 · 条件符合".to_string()
            } else if self.gpus.is_empty() {
                "正在读取 DXGI…".to_string()
            } else {
                "未检测到 RDNA4".to_string()
            };
            draw_device_card(ui.painter(), rect, name, &detail, supported);
        });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, page: Page, label: &str, icon: NavIcon) {
        let selected = self.page == page && self.screen == Screen::Select;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::accesskit::Role::Tab,
                self.screen != Screen::Installing,
                selected,
                label,
            )
        });
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 10.0, egui::Color32::from_rgb(21, 55, 73));
            painter.rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 83, 106)),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 10.0), egui::vec2(3.0, 24.0)),
                2.0,
                CYAN,
            );
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 8.0), egui::vec2(7.0, 28.0)),
                3.5,
                egui::Color32::from_rgba_unmultiplied(119, 227, 234, 22),
            );
        } else if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, 10.0, egui::Color32::from_rgb(28, 40, 58));
        }
        let color = if selected { TEXT } else { TEXT2 };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 23.0, rect.center().y),
            egui::vec2(19.0, 19.0),
        );
        draw_nav_icon(
            painter,
            icon_rect,
            icon,
            if selected { CYAN } else { TEXT2 },
        );
        painter.text(
            egui::pos2(rect.left() + 45.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font(13.0),
            color,
        );
        if response.clicked() && self.screen != Screen::Installing && !self.maintenance_busy {
            self.page = page;
            if self.screen == Screen::Done {
                self.screen = Screen::Select;
                self.report = None;
            }
        }
        ui.add_space(5.0);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let section = match (self.screen, self.page) {
            (Screen::Installing, _) => "安装进度",
            (Screen::Done, _) => "安装完成",
            (_, Page::Install) => "安装中心",
            (_, Page::Maintain) => "维护与管理",
            (_, Page::Settings) => "偏好设置",
        };
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new("OptiScaler NR").size(14.0).color(TEXT2));
            ui.label(egui::RichText::new("/").size(14.0).color(MUTED));
            ui.label(egui::RichText::new(section).size(14.0).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(13, 21, 33))
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10, 5))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(5.0, 5.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 2.5, GREEN);
                            ui.label(
                                egui::RichText::new(format!("固定方案 · v{APP_VERSION}"))
                                    .size(12.0)
                                    .color(TEXT2),
                            );
                        });
                    });
            });
        });
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, LINE),
        );
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter()
            .hline(rect.x_range(), rect.top(), egui::Stroke::new(1.0, LINE));
        let (dot_color, headline) = if self.gpus.is_empty() {
            (AMBER, "正在检测环境")
        } else if self.can_install() {
            (GREEN, "环境检测通过 · 可以继续")
        } else {
            (RED, "当前显卡不符合固定方案要求")
        };
        let label_width = (ui.available_width() - 220.0).max(120.0);
        ui.horizontal_centered(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(label_width, ACTION_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(ACTION_HEIGHT);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, dot_color);
                        ui.label(egui::RichText::new(headline).size(14.0).color(TEXT));
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(15.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&self.status).size(12.0).color(MUTED),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&self.status);
                    });
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled =
                    self.selected_game().is_some() && self.can_install() && !self.app_update_busy;
                let label = match self.selected_game() {
                    Some(game) if self.game_needs_update(game) => "更新到此游戏  →",
                    Some(game) if self.install_status(game).starts_with("已安装 ") => {
                        "重新安装固定方案  →"
                    }
                    Some(game) if self.install_status(game) == "安装未完成" => {
                        "重新安装固定方案  →"
                    }
                    Some(game) if self.install_status(game) == "外部安装" => {
                        "覆盖安装到此游戏  →"
                    }
                    Some(_) => "安装到此游戏  →",
                    None => "安装到此游戏  →",
                };
                if primary_button(ui, label, enabled, 208.0) {
                    self.install_selected();
                }
            });
        });
    }

    fn install_page(&mut self, ui: &mut egui::Ui) {
        self.info_cards(ui);
        ui.add_space(8.0);
        let mut manual = false;
        let mut rescan = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("选择目标游戏")
                    .font(bold(19.0))
                    .color(TEXT),
            );
            egui::ComboBox::from_id_salt("install_models")
                .selected_text(match self.settings.models {
                    ModelSelection::Original => "模型：310.8.0",
                    ModelSelection::Small => "模型：310.9.16",
                    ModelSelection::Both => "模型：两个都安装",
                })
                .width(140.0)
                .show_ui(ui, |ui| {
                    for models in [
                        ModelSelection::Original,
                        ModelSelection::Small,
                        ModelSelection::Both,
                    ] {
                        ui.selectable_value(&mut self.settings.models, models, models.label());
                    }
                })
                .response
                .on_hover_text("选择要安装的模型；双模型首次安装默认启用新模型；已有选择会保留，可在游戏内切换");
            ui.add_space(4.0);
            if self.scanning_games {
                ui.spinner();
                ui.label(egui::RichText::new("正在扫描游戏…").size(12.0).color(TEXT2));
            } else {
                status_badge(
                    ui,
                    &format!(
                        "{} GAME{}",
                        self.games.len(),
                        if self.games.len() == 1 { "" } else { "S" }
                    ),
                    egui::Color32::from_rgb(21, 36, 48),
                    CYAN,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled_ui(!self.scanning_games, |ui| ghost_button(ui, "手动添加 EXE"))
                    .inner
                    .clicked()
                {
                    manual = true;
                }
                if ui
                    .add_enabled_ui(!self.scanning_games, |ui| ghost_button(ui, "重新扫描"))
                    .inner
                    .clicked()
                {
                    rescan = true;
                }
            });
        });
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new("兼容性提醒：候选游戏仍需游戏内验证；操作前请退出游戏，联网游戏注意反作弊风险。").size(12.0).color(TEXT2)).wrap());
        ui.add_space(8.0);

        if self.games.is_empty() {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 76.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 10.0, CARD_ALT);
            painter.rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(1.0, LINE),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if self.scanning_games {
                    "正在扫描已安装游戏，请稍候…"
                } else {
                    "没有自动发现游戏。重新扫描，或手动选择一个游戏 EXE。"
                },
                font(11.5),
                MUTED,
            );
        } else {
            let mut clicked = None;
            let list_height = (ui.available_height() - CONTENT_INSET).max(0.0);
            let row_height = ((list_height - GAME_ROW_GAP * 4.0) / 5.0)
                .floor()
                .clamp(56.0, 84.0);
            egui::ScrollArea::vertical()
                .id_salt("game-list")
                .max_height(list_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, game) in self.games.iter().enumerate() {
                        let status = if self.game_needs_update(game) {
                            format!("{} · 可更新", self.install_status(game))
                        } else {
                            self.install_status(game)
                        };
                        if game_card(
                            ui,
                            game,
                            &status,
                            self.selected_game == Some(index),
                            row_height,
                        ) {
                            clicked = Some(index);
                        }
                        if index + 1 < self.games.len() {
                            ui.add_space(GAME_ROW_GAP);
                        }
                    }
                });
            if let Some(index) = clicked {
                self.selected_game = Some(index);
            }
        }

        if manual {
            self.choose_game_exe();
        }
        if rescan {
            self.refresh_games();
        }
    }

    fn info_cards(&mut self, ui: &mut egui::Ui) {
        let gpu = self
            .gpus
            .iter()
            .find(|gpu| gpu.mochizuki_class.supports_fixed_profile())
            .or_else(|| self.gpus.iter().find(|gpu| !gpu.software));
        let gpu_name = gpu.map(|gpu| gpu.name.as_str()).unwrap_or("正在检测…");
        let gpu_status = if self.can_install() {
            "RDNA4 检测通过"
        } else if self.gpus.is_empty() {
            "正在读取 DXGI…"
        } else {
            "未检测到受支持的 RDNA4"
        };
        let up_value = match &self.upstream {
            Some(release) => format!(
                "{} · {}",
                release.tag,
                if self.upstream_error.is_some() || self.checking_updates {
                    "缓存"
                } else {
                    "Stable"
                },
            ),
            None if self.checking_updates => "检查中…".to_string(),
            None => "检查失败".to_string(),
        };

        let gap = 10.0;
        let width = (ui.available_width() - gap * 2.0) / 3.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            info_card(
                ui,
                width,
                CardIcon::Gpu,
                "显卡兼容性",
                gpu_name,
                gpu_status,
                if self.can_install() { GREEN } else { AMBER },
            );
            info_card(
                ui,
                width,
                CardIcon::Download,
                "组件主包",
                &up_value,
                "上游当前最新版本 · 部署时下载",
                VIOLET,
            );
            info_card(
                ui,
                width,
                CardIcon::Spark,
                "固定部署方案",
                "NR + Mochizuki + XeFG",
                "默认 XeFG · 2X",
                BLUE,
            );
        });
    }

    fn progress_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(30.0);
        let width = ui.available_width().min(660.0);
        let margin = ((ui.available_width() - width) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.vertical(|ui| {
                ui.set_width(width);
                eyebrow(ui, "SECURE DEPLOYMENT");
                ui.add_space(14.0);
                let game = self
                    .selected_game()
                    .map(|game| game.name.as_str())
                    .unwrap_or("游戏");
                ui.label(
                    egui::RichText::new(format!("正在为 {game} 部署组件"))
                        .font(bold(23.0))
                        .color(TEXT),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "正在按固定方案处理文件。安装期间请勿启动游戏，也不要关闭安装器。",
                    )
                    .size(12.0)
                    .color(TEXT2),
                );
                ui.add_space(22.0);
                let percent = self
                    .progress
                    .fraction
                    .map(|fraction| format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0))
                    .unwrap_or_else(|| "…".to_string());
                ui.label(
                    egui::RichText::new(percent)
                        .font(bold(38.0))
                        .color(egui::Color32::from_rgb(197, 251, 250)),
                );
                ui.add_space(10.0);
                progress_bar(ui, width, self.progress.fraction);
                ui.add_space(24.0);
                let current = progress_stage_index(&self.progress.stage);
                for (index, label) in PROGRESS_STEPS.iter().enumerate() {
                    let state = if index < current {
                        StepState::Done
                    } else if index == current {
                        StepState::Live
                    } else {
                        StepState::Pending
                    };
                    draw_progress_step(ui, index + 1, label, state);
                    ui.add_space(9.0);
                }
                ui.add_space(16.0);
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(28, 32, 50))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 56, 78)))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(13, 11))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, _) = ui
                                .allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            ui.painter().circle_stroke(
                                icon.center(),
                                7.0,
                                egui::Stroke::new(1.0, AMBER),
                            );
                            ui.painter().text(
                                icon.center(),
                                egui::Align2::CENTER_CENTER,
                                "!",
                                font(8.0),
                                AMBER,
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(
                                        "若发生错误，安装会停止且不自动回滚；请卸载后重新安装。下载与校验会自动重试。",
                                    )
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(168, 170, 188)),
                                )
                                .wrap(),
                            );
                        });
                    });
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new(&self.progress.detail)
                        .size(10.5)
                        .color(MUTED),
                );
            });
        });
    }

    fn done_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let (icon, _) = ui.allocate_exact_size(egui::vec2(90.0, 90.0), egui::Sense::hover());
            ui.painter().circle_filled(
                icon.center(),
                43.0,
                egui::Color32::from_rgb(21, 48, 59),
            );
            ui.painter().circle_stroke(
                icon.center(),
                43.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(92, 216, 193)),
            );
            draw_check(
                ui.painter(),
                icon.center(),
                34.0,
                egui::Color32::from_rgb(167, 255, 242),
            );
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(
                    self.report
                        .as_ref()
                        .map(|report| report.title.as_str())
                        .unwrap_or("安装完成"),
                )
                .font(bold(26.0))
                .color(TEXT),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("固定方案已写入游戏目录。进入游戏后按下面的步骤验收。")
                    .size(12.0)
                    .color(egui::Color32::from_rgb(145, 168, 188)),
            );
            ui.add_space(24.0);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(19, 30, 45))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 64, 86)))
                .corner_radius(12.0)
                .inner_margin(egui::Margin::symmetric(22, 18))
                .show(ui, |ui| {
                    ui.set_width((ui.available_width() - 44.0).min(630.0));
                    ui.label(
                        egui::RichText::new("实际安装成功后，可以这样验收：")
                            .font(bold(12.0))
                            .color(TEXT),
                    );
                    ui.add_space(8.0);
                    for line in [
                        "01 · 启动游戏，在设置中开启 DLSS 帧生成。",
                        "02 · 按 Insert 打开 OptiScaler，检查后端是否为 mochizuki、输出是否为 XeFG。",
                        "03 · 首次进入游戏场景后退出并重新启动游戏，让 HighResMV 检测生效。",
                    ] {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(line)
                                .size(11.5)
                                .color(egui::Color32::from_rgb(168, 188, 208)),
                        );
                    }
                });
            ui.add_space(22.0);
            ui.allocate_ui_with_layout(egui::vec2(330.0, 42.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if secondary_button(ui, "返回安装中心", 148.0).clicked() {
                    self.screen = Screen::Select;
                    self.page = Page::Install;
                    self.report = None;
                }
                ui.add_space(10.0);
                if primary_button(ui, "查看维护功能  →", true, 180.0) {
                    self.screen = Screen::Select;
                    self.page = Page::Maintain;
                    self.report = None;
                }
            });
            if let Some(report) = &self.report {
                ui.add_space(18.0);
                egui::CollapsingHeader::new("查看安装报告").show(ui, |ui| {
                    for line in &report.lines {
                        ui.label(egui::RichText::new(line).size(11.0).color(TEXT2));
                    }
                });
            }
        });
    }

    fn maintain_page(&mut self, ui: &mut egui::Ui) {
        let game_label = self
            .selected_game()
            .map(|game| game.name.clone())
            .unwrap_or_else(|| "未选择".to_string());
        let mut selected = self.selected_game;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                eyebrow(ui, "INSTALLATION CONTROL");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("维护与管理")
                        .font(bold(24.0))
                        .color(TEXT),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "检查、修复或撤销已部署的组件。所有变更均基于安装记录进行。",
                    )
                    .size(12.0)
                    .color(TEXT2),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.games.is_empty() {
                    ui.label(
                        egui::RichText::new("当前游戏：未发现")
                            .size(10.5)
                            .color(MUTED),
                    );
                } else {
                    egui::ComboBox::from_id_salt("maintain-game")
                        .selected_text(
                            egui::RichText::new(format!("当前游戏：{game_label}"))
                                .size(10.5)
                                .color(TEXT2),
                        )
                        .width(172.0)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for (index, game) in self.games.iter().enumerate() {
                                if ui
                                    .selectable_label(selected == Some(index), &game.name)
                                    .clicked()
                                {
                                    selected = Some(index);
                                }
                            }
                        });
                }
            });
        });
        if selected != self.selected_game {
            self.selected_game = selected;
        }
        if self.maintenance_busy {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    egui::RichText::new("正在执行维护操作…")
                        .size(10.5)
                        .color(CYAN),
                );
            });
        }

        ui.add_space(22.0);
        let gap = 13.0;
        let width = (ui.available_width() - gap) / 2.0;
        let enabled =
            self.selected_game().is_some() && !self.maintenance_busy && !self.app_update_busy;
        let managed = enabled
            && self
                .selected_game()
                .is_some_and(|game| is_managed_status(&self.install_status(game)));
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if maintenance_card(
                ui,
                width,
                MaintIcon::Shield,
                "完整性检查",
                "验证文件哈希、代理 DLL 与固定配置，不修改游戏。",
                managed,
                None,
            ) {
                action = Some(Action::Check);
            }
            if maintenance_card(
                ui,
                width,
                MaintIcon::Tool,
                "修复组件",
                "补齐缺失或损坏的文件，同时保留用户设置。",
                enabled && self.can_install(),
                None,
            ) {
                action = Some(Action::Repair);
            }
        });
        ui.add_space(gap);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if maintenance_card(
                ui,
                width,
                MaintIcon::Download,
                "更新方案",
                "部署来自固定上游的最新兼容版本。",
                enabled && self.can_install(),
                None,
            ) {
                action = Some(Action::Update);
            }
            if maintenance_card(
                ui,
                width,
                MaintIcon::Shield,
                "安装 REFramework",
                "为 PRAGMATA 等 RE Engine 游戏安装 dinput8.dll，修复启动后自动关闭。",
                enabled,
                None,
            ) {
                action = Some(Action::InstallReframework);
            }
        });
        ui.add_space(gap);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if maintenance_card(
                ui,
                width,
                MaintIcon::Power,
                "关闭 XeFG 插帧",
                "暂时停用帧生成插件，方便排查游戏问题。",
                managed,
                None,
            ) {
                action = Some(Action::DisableFrameGen);
            }
            if maintenance_card(
                ui,
                width,
                MaintIcon::Trash,
                "安全卸载",
                "删除本项目注入的受管文件，不保留旧版本备份。",
                managed,
                None,
            ) {
                action = Some(Action::Uninstall);
            }
        });
        if let Some(action) = action
            && let Some(game) = self.selected_game().cloned()
        {
            if matches!(action, Action::Check) {
                self.run_maintenance(game, action);
            } else {
                self.pending_maintenance = Some((game, action));
            }
        }

        ui.add_space(22.0);
        let (divider, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().hline(
            divider.x_range(),
            divider.center().y,
            egui::Stroke::new(1.0, LINE),
        );
        ui.add_space(18.0);
        egui::Frame::NONE
            .fill(egui::Color32::from_rgb(28, 32, 50))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 56, 78)))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(13, 11))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                    ui.painter()
                        .circle_stroke(icon.center(), 7.0, egui::Stroke::new(1.0, AMBER));
                    ui.painter().text(
                        icon.center(),
                        egui::Align2::CENTER_CENTER,
                        "!",
                        font(8.0),
                        AMBER,
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "检查、关闭插帧和卸载需本项目的安装记录；修复与更新仍需支持的显卡。安装失败后请卸载重装，安装器不保留回滚文件或组件缓存。",
                            )
                            .size(11.0)
                            .color(egui::Color32::from_rgb(168, 170, 188)),
                        )
                        .wrap(),
                    );
                });
            });

        ui.add_space(22.0);
        section_label(ui, "更新状态");
        egui::Frame::NONE
            .fill(CARD)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("组件主包").size(10.0).color(MUTED));
                        if let Some(upstream) = &self.upstream {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {} · {}",
                                    upstream.tag,
                                    upstream.asset.name,
                                    timestamp(upstream.checked_at_ms)
                                ))
                                .size(11.0)
                                .color(TEXT2),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("尚未取得上游版本")
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.checking_updates {
                            "检查中…"
                        } else {
                            "检查全部更新"
                        };
                        let response = ui
                            .add_enabled_ui(!self.checking_updates, |ui| ghost_button(ui, label))
                            .inner;
                        if response.clicked() {
                            self.check_updates(true);
                        }
                        response.on_hover_text("同时检查上游组件主包和安装器自身版本");
                    });
                });
                if let Some(result) = &self.last_update_check {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(result)
                            .size(10.5)
                            .color(if self.checking_updates { CYAN } else { TEXT2 }),
                    );
                }
                if let Some(update) = &self.app_update {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("安装器新版本：{}", update.version))
                                    .size(11.0)
                                    .color(TEXT),
                            );
                            if !update.notes.is_empty() {
                                ui.label(
                                    egui::RichText::new(truncate(&update.notes, 90))
                                        .size(9.5)
                                        .color(MUTED),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    !self.app_update_busy && !self.maintenance_busy,
                                    egui::Button::new("下载并更新安装器"),
                                )
                                .clicked()
                            {
                                self.app_update_phase = Some(AppUpdatePhase::Prompt);
                                self.app_update_error = None;
                            }
                        });
                    });
                }
            });

        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("固定方案：{}", FIXED_PROFILE))
                .size(9.5)
                .color(MUTED),
        );
        ui.add_space(12.0);
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        eyebrow(ui, "PREFERENCES & SOURCES");
        ui.add_space(8.0);
        ui.label(egui::RichText::new("偏好设置").font(bold(24.0)).color(TEXT));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("保持默认的一键安装流程；高级来源设置仅用于离线安装或排障。")
                .size(12.0)
                .color(TEXT2),
        );
        ui.add_space(24.0);

        let main_path = self.draft.local_upstream_source.clone();
        let extras_path = self.draft.local_extras_source.clone();
        let data_dir = self
            .paths
            .as_ref()
            .ok()
            .map(|paths| paths.root.display().to_string())
            .unwrap_or_else(|| "%LOCALAPPDATA%\\OptiScalerNR".to_string());
        let mut pick_main = false;
        let mut pick_extras = false;
        let mut clear_main = false;
        let mut clear_extras = false;

        egui::Frame::NONE
            .fill(CARD)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(12.0)
            .show(ui, |ui| {
                settings_row(
                    ui,
                    "部署配置",
                    "使用项目锁定的唯一方案，避免不兼容组合。",
                    |ui| {
                        ui.label(
                            egui::RichText::new("NR · Mochizuki · XeFG")
                                .size(11.5)
                                .color(TEXT2),
                        );
                        ui.label(egui::RichText::new("已锁定").size(9.5).color(CYAN));
                    },
                );
                ui.separator();
                let main_desc = main_path
                    .as_ref()
                    .map(|path| {
                        format!(
                            "可选。留空时默认使用固定上游 Release。已选择：{}",
                            truncate(&path.display().to_string(), 56)
                        )
                    })
                    .unwrap_or_else(|| "可选。留空时默认使用固定上游 Release。".to_string());
                settings_row(ui, "本地主包", &main_desc, |ui| {
                    if secondary_button(ui, "选择 ZIP…", 110.0).clicked() {
                        pick_main = true;
                    }
                    if main_path.is_some() && ghost_button(ui, "清除").clicked() {
                        clear_main = true;
                    }
                });
                ui.separator();
                let extras_desc = extras_path
                    .as_ref()
                    .map(|path| {
                        format!(
                            "可选。留空时从 Cloudflare 下载固定资源。已选择：{}",
                            truncate(&path.display().to_string(), 56)
                        )
                    })
                    .unwrap_or_else(|| "可选。留空时从 Cloudflare 下载固定资源。".to_string());
                settings_row(ui, "本地资源目录", &extras_desc, |ui| {
                    if secondary_button(ui, "选择目录…", 110.0).clicked() {
                        pick_extras = true;
                    }
                    if extras_path.is_some() && ghost_button(ui, "清除").clicked() {
                        clear_extras = true;
                    }
                });
                ui.separator();
                settings_row(
                    ui,
                    "应用数据目录",
                    "缓存、安装记录及日志的默认保存位置。",
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(truncate(&data_dir, 46))
                                    .size(10.5)
                                    .color(TEXT2),
                            )
                            .truncate(),
                        );
                    },
                );
            });
        if pick_main {
            self.choose_upstream_source();
        }
        if pick_extras {
            self.choose_extras_source();
        }
        if clear_main {
            self.draft.local_upstream_source = None;
        }
        if clear_extras {
            self.draft.local_extras_source = None;
        }

        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("修改会在下一次安装或维护时生效。")
                .size(10.5)
                .color(MUTED),
        );
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if primary_button(ui, "保存设置", true, 140.0) {
                self.save_settings();
            }
            if self.status == "设置已保存" {
                ui.label(egui::RichText::new("设置已保存").size(11.0).color(GREEN));
            }
            ui.label(
                egui::RichText::new(format!("v{APP_VERSION}"))
                    .size(9.0)
                    .color(MUTED),
            );
        });
        ui.add_space(20.0);

        egui::CollapsingHeader::new(egui::RichText::new("高级来源设置").size(11.0).color(TEXT2))
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(8.0);
                field_label(ui, "安装器发布仓库");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.release_repo)
                        .desired_width(f32::INFINITY)
                        .font(font(11.0)),
                );
                ui.add_space(10.0);
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "固定资源由安装器内置清单校验，通过 Cloudflare 分文件下载。",
                    )
                    .size(9.5)
                    .color(MUTED),
                );
            });

        ui.add_space(18.0);
        ui.label(
            egui::RichText::new(format!("{} · {}", APP_NAME, FIXED_PROFILE))
                .size(9.0)
                .color(MUTED),
        );
    }
}

impl App {
    fn render(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.drain_events();

        egui::Panel::left("sidebar")
            .exact_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(SIDEBAR).inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| self.sidebar(ui));

        egui::Panel::top("breadcrumb")
            .exact_size(62.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(35, 0)),
            )
            .show(ui, |ui| self.top_bar(ui));

        if self.screen == Screen::Select && self.page == Page::Install {
            egui::Panel::bottom("footer")
                .exact_size(80.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::NONE
                        .fill(CARD)
                        .inner_margin(egui::Margin::symmetric(35, 0)),
                )
                .show(ui, |ui| self.bottom_bar(ui));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(egui::Margin {
                left: 35,
                right: 35,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                if let Some(error) = self.error.clone() {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(40, 21, 25))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(92, 40, 46)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(error)
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(245, 160, 160)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("关闭")
                                                        .size(9.5)
                                                        .color(TEXT2),
                                                )
                                                .fill(egui::Color32::TRANSPARENT)
                                                .stroke(egui::Stroke::NONE),
                                            )
                                            .clicked()
                                        {
                                            self.error = None;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(10.0);
                }

                if self.screen == Screen::Select && self.page == Page::Install {
                    ui.add_space(12.0);
                    self.install_page(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("content")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(28.0);
                        match self.screen {
                            Screen::Installing => self.progress_page(ui),
                            Screen::Done => self.done_page(ui),
                            Screen::Select => match self.page {
                                Page::Install => self.install_page(ui),
                                Page::Maintain => self.maintain_page(ui),
                                Page::Settings => self.settings_page(ui),
                            },
                        }
                    });
            });
        if let Some((game, action)) = self.pending_maintenance.clone() {
            let mut confirm = false;
            let mut cancel = false;
            let description = match action {
                Action::Uninstall => {
                    "将删除本项目受管的注入文件。当前安装器不保留旧文件备份，请先退出游戏。"
                }
                Action::DisableFrameGen => "将修改游戏目录中的配置，关闭 XeFG 插帧。请先退出游戏。",
                Action::InstallReframework => {
                    "将从 praydog/REFramework-nightly 下载最新版本，并覆盖游戏目录中的 dinput8.dll。请先退出游戏。"
                }
                _ => {
                    "将重新部署固定方案；同名文件会直接覆盖。当前安装器不保留旧文件备份，请先退出游戏。"
                }
            };
            let modal = egui::Modal::new(egui::Id::unique("maintenance-confirm"))
                .frame(
                    egui::Frame::NONE
                        .fill(CARD)
                        .stroke(egui::Stroke::new(1.0, LINE))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::same(DIALOG_PADDING)),
                )
                .show(&ctx, |ui| {
                    let viewport = ctx.content_rect();
                    ui.set_width(
                        DIALOG_WIDTH.min(viewport.width() - 2.0 * (DIALOG_PADDING as f32 + 24.0)),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}？", action.label()))
                            .font(bold(20.0))
                            .color(TEXT),
                    );
                    ui.add_space(14.0);
                    egui::ScrollArea::vertical()
                        .id_salt("confirmation-body")
                        .max_height((viewport.height() - 232.0).clamp(64.0, 104.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::NONE
                                .fill(BG)
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new("目标游戏").size(12.0).color(TEXT2),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&game.name)
                                                .font(bold(15.0))
                                                .color(TEXT),
                                        )
                                        .wrap(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(game.exe.display().to_string())
                                                .size(12.0)
                                                .color(TEXT2),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                    ui.add_space(10.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 48.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_height(48.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(description).size(13.0).color(TEXT2),
                                )
                                .wrap(),
                            );
                        },
                    );
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(
                            (ui.available_width() - 296.0 - ui.spacing().item_spacing.x).max(0.0),
                        );
                        cancel = secondary_button(ui, "取消", 148.0).clicked();
                        confirm = primary_button(ui, action.label(), true, 148.0);
                    });
                });
            if confirm {
                self.pending_maintenance = None;
                self.run_maintenance(game, action);
            } else if cancel || modal.should_close() {
                self.pending_maintenance = None;
            }
        }
        self.app_update_modal(&ctx);
        self.maintenance_report_modal(&ctx);
        ctx.request_repaint_after(Duration::from_millis(120));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}

fn spawn_worker(paths: Result<AppPaths, String>) -> (Sender<Command>, Receiver<Event>) {
    let (command_tx, command_rx) = channel::<Command>();
    let (event_tx, event_rx) = channel::<Event>();
    std::thread::spawn(move || {
        if let Ok(paths) = paths.as_ref()
            && let Err(error) = install::cleanup_installer_files(paths)
        {
            let _ = event_tx.send(Event::Status(format!("清理旧缓存失败：{error}")));
        }
        for command in command_rx {
            match command {
                Command::Refresh => {
                    let gpu_result = gpu::detect_gpus();
                    let game_result = games::discover_games();
                    let _ = event_tx.send(Event::Gpus(gpu_result));
                    let _ = event_tx.send(Event::Games(game_result));
                }
                Command::CheckUpdates(settings, force) => {
                    let paths = match &paths {
                        Ok(paths) => paths,
                        Err(error) => {
                            let _ = event_tx.send(Event::Checked {
                                upstream: Err(error.clone()),
                                app_update: Ok(None),
                                warning: None,
                            });
                            continue;
                        }
                    };
                    // 安装器与主包分别判定：主包清单错误只影响主包，安装器更新照常触发。
                    let (upstream, app_update, warning) =
                        match crate::catalog::check(paths, &settings, force) {
                            Ok(catalog) => {
                                let upstream =
                                    catalog.upstream.clone().map(Box::new).ok_or_else(|| {
                                        catalog
                                            .upstream_error
                                            .clone()
                                            .unwrap_or_else(|| "更新清单未包含主包信息".into())
                                    });
                                let app_update = self_update::available(&catalog.installer);
                                (upstream, app_update, catalog.warning)
                            }
                            Err(error) => (Err(error.clone()), Err(error), None),
                        };
                    let _ = event_tx.send(Event::Checked {
                        upstream,
                        app_update,
                        warning,
                    });
                }
                Command::Install {
                    game,
                    settings,
                    upstream,
                    main_package_only,
                } => {
                    let result = match &paths {
                        Ok(paths) => run_install(
                            paths,
                            &settings,
                            &game,
                            upstream,
                            main_package_only,
                            &event_tx,
                        ),
                        Err(error) => Err(error.clone()),
                    };
                    let _ = event_tx.send(Event::Installed(result));
                }
                Command::Maintain {
                    game,
                    action,
                    settings,
                    upstream,
                } => {
                    let paths = match &paths {
                        Ok(paths) => paths,
                        Err(error) => {
                            let _ = event_tx.send(Event::Maintained {
                                action,
                                result: Err(error.clone()),
                            });
                            continue;
                        }
                    };
                    let result = match action {
                        Action::Repair | Action::Update => install::load_record(paths, &game)
                            .and_then(|record| {
                                let mut settings = settings;
                                settings.models = record.models;
                                run_install(paths, &settings, &game, upstream, false, &event_tx)
                            }),
                        Action::Check => install::check_install(paths, &game),
                        Action::Uninstall => install::uninstall(paths, &game),
                        Action::DisableFrameGen => install::disable_frame_gen(paths, &game),
                        Action::InstallReframework => {
                            crate::reframework::install(paths, &settings, &game, |progress| {
                                let _ = event_tx.send(Event::Status(progress.detail));
                            })
                        }
                    };
                    let _ = event_tx.send(Event::Maintained { action, result });
                }
                Command::ApplyAppUpdate(update) => {
                    let result = match &paths {
                        Ok(paths) => self_update::apply(paths, &update, |progress| {
                            let _ = event_tx.send(Event::AppUpdateProgress(progress));
                        }),
                        Err(error) => Err(error.clone()),
                    };
                    let _ = event_tx.send(Event::AppUpdateApplied(result));
                }
            }
        }
    });
    (command_tx, event_rx)
}

fn run_install(
    paths: &AppPaths,
    settings: &Settings,
    game: &GameInfo,
    upstream: Option<UpstreamRelease>,
    main_package_only: bool,
    event_tx: &Sender<Event>,
) -> Result<MaintenanceReport, String> {
    let components = assets::prepare(paths, settings, upstream, !main_package_only, |progress| {
        let _ = event_tx.send(Event::Progress(progress));
    })?;
    let result = install::install(paths, game, &components, |progress| {
        let _ = event_tx.send(Event::Progress(progress));
    });
    if let Err(error) = install::cleanup_installer_files(paths) {
        let _ = event_tx.send(Event::Status(format!("清理组件缓存失败：{error}")));
    }
    result
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/msyh.ttc") {
        fonts.font_data.insert(
            "cjk".to_string(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_string());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "cjk".to_string());
    }
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/msyhbd.ttc") {
        fonts.font_data.insert(
            "cjk-bold".to_string(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
    }
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/segoeui.ttf") {
        fonts.font_data.insert(
            "latin".to_string(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            family.insert(0, "latin".to_string());
        }
    }
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/segoeuib.ttf") {
        fonts.font_data.insert(
            "latin-bold".to_string(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
    }
    let bold_fallbacks = [
        "latin-bold".to_string(),
        "cjk-bold".to_string(),
        "cjk".to_string(),
        "Ubuntu-Light".to_string(),
    ]
    .into_iter()
    .filter(|name| fonts.font_data.contains_key(name))
    .collect();
    fonts
        .families
        .insert(egui::FontFamily::Name("Bold".into()), bold_fallbacks);
    ctx.set_fonts(fonts);
}

fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = CARD;
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(9, 16, 26);
    style.visuals.text_edit_bg_color = Some(egui::Color32::from_rgb(9, 16, 26));
    style.visuals.faint_bg_color = egui::Color32::from_rgb(13, 21, 33);
    style.visuals.window_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.window_corner_radius = 10.into();
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(27, 74, 91);
    style.visuals.hyperlink_color = CYAN;
    style.visuals.warn_fg_color = AMBER;
    style.visuals.error_fg_color = RED;
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT2);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT2);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(41, 58, 78);
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(13, 23, 36);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(18, 32, 48);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(18, 32, 48);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN_DIM);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(22, 42, 56);
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(22, 42, 56);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, CYAN);
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.scroll.floating = true;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating_width = 3.0;
    style.spacing.scroll.floating_allocated_width = 0.0;
    style.spacing.scroll.foreground_color = false;
    style.spacing.scroll.dormant_handle_opacity = 0.5;
    style.spacing.scroll.active_handle_opacity = 0.85;
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn font(size: f32) -> egui::FontId {
    egui::FontId::proportional(size)
}

fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("Bold".into()))
}

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn truncated_galley(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.layout_job(job)
}

fn draw_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    from: f32,
    to: f32,
    width: f32,
    color: egui::Color32,
) {
    let points: Vec<egui::Pos2> = (0..=48)
        .map(|index| {
            let t = index as f32 / 48.0;
            let angle = from + (to - from) * t;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
}

fn draw_check(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    let stroke = egui::Stroke::new((size * 0.12).max(1.2), color);
    let start = center + egui::vec2(-size * 0.32, 0.0);
    let elbow = center + egui::vec2(-size * 0.08, size * 0.26);
    let end = center + egui::vec2(size * 0.38, -size * 0.3);
    painter.line_segment([start, elbow], stroke);
    painter.line_segment([elbow, end], stroke);
}

fn draw_logo(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 10.0, egui::Color32::from_rgb(11, 22, 37));
    painter.rect_stroke(
        rect,
        10.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 66, 88)),
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(
        rect.center(),
        15.0,
        egui::Color32::from_rgba_unmultiplied(73, 197, 220, 18),
    );
    painter.circle_stroke(rect.center(), 15.0, egui::Stroke::new(1.0, CYAN_DIM));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "N",
        bold(17.0),
        CYAN,
    );
}

fn draw_nav_icon(painter: &egui::Painter, rect: egui::Rect, icon: NavIcon, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    let left = rect.left();
    let right = rect.right();
    let top = rect.top();
    let bottom = rect.bottom();
    let mid = rect.center().x;
    match icon {
        NavIcon::Home => {
            painter.line_segment(
                [egui::pos2(left, rect.center().y), egui::pos2(mid, top)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(mid, top), egui::pos2(right, rect.center().y)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(left + 2.0, rect.center().y),
                    egui::pos2(left + 2.0, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(right - 2.0, rect.center().y),
                    egui::pos2(right - 2.0, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(left + 2.0, bottom),
                    egui::pos2(right - 2.0, bottom),
                ],
                stroke,
            );
        }
        NavIcon::Layers => {
            let diamond = |y: f32| {
                vec![
                    egui::pos2(mid, y - 4.0),
                    egui::pos2(right, y),
                    egui::pos2(mid, y + 4.0),
                    egui::pos2(left, y),
                    egui::pos2(mid, y - 4.0),
                ]
            };
            painter.add(egui::Shape::line(diamond(top + 5.0), stroke));
            painter.add(egui::Shape::line(diamond(top + 11.0), stroke));
            painter.add(egui::Shape::line(diamond(top + 17.0), stroke));
        }
        NavIcon::Sliders => {
            for (index, knob) in [0.2, 0.72, 0.4].into_iter().enumerate() {
                let y = top + 2.0 + index as f32 * 6.0;
                painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], stroke);
                painter.circle_filled(egui::pos2(left + (right - left) * knob, y), 2.0, color);
            }
        }
    }
}

fn draw_device_card(
    painter: &egui::Painter,
    rect: egui::Rect,
    name: &str,
    detail: &str,
    supported: bool,
) {
    painter.rect_filled(rect, 9.0, egui::Color32::from_rgb(11, 19, 30));
    painter.rect_stroke(
        rect,
        9.0,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    let left = rect.left() + 12.0;
    painter.text(
        egui::pos2(left, rect.top() + 16.0),
        egui::Align2::LEFT_CENTER,
        "DEVICE STATUS",
        font(8.0),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - 12.0, rect.top() + 16.0),
        egui::Align2::RIGHT_CENTER,
        if supported { "PASS" } else { "WAIT" },
        bold(8.0),
        if supported { GREEN } else { AMBER },
    );
    painter.text(
        egui::pos2(left, rect.top() + 40.0),
        egui::Align2::LEFT_CENTER,
        truncate(name, 24),
        bold(11.5),
        TEXT,
    );
    painter.circle_filled(
        egui::pos2(left + 5.0, rect.top() + 61.0),
        4.0,
        egui::Color32::from_rgb(17, 31, 45),
    );
    draw_check(
        painter,
        egui::pos2(left + 5.0, rect.top() + 61.0),
        7.0,
        if supported { GREEN } else { AMBER },
    );
    painter.text(
        egui::pos2(left + 15.0, rect.top() + 61.0),
        egui::Align2::LEFT_CENTER,
        detail,
        font(9.0),
        TEXT2,
    );
    painter.hline(
        left..=rect.right() - 12.0,
        rect.top() + 78.0,
        egui::Stroke::new(1.0, LINE),
    );
    painter.text(
        egui::pos2(left, rect.bottom() - 18.0),
        egui::Align2::LEFT_CENTER,
        "固定方案",
        font(8.5),
        MUTED,
    );
    painter.text(
        egui::pos2(rect.right() - 12.0, rect.bottom() - 18.0),
        egui::Align2::RIGHT_CENTER,
        "RDNA4",
        font(8.5),
        MUTED,
    );
}

#[derive(Clone, Copy)]
enum CardIcon {
    Gpu,
    Download,
    Spark,
}

fn info_card(
    ui: &mut egui::Ui,
    width: f32,
    icon: CardIcon,
    title: &str,
    value: &str,
    status: &str,
    accent: egui::Color32,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 96.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, CARD);
    painter.rect_stroke(
        rect,
        10.0,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    let icon_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 14.0), egui::vec2(34.0, 34.0));
    painter.rect_filled(icon_rect, 8.0, egui::Color32::from_rgb(16, 27, 43));
    draw_card_icon(painter, icon_rect, icon, accent);
    let left = rect.left() + 58.0;
    painter.text(
        egui::pos2(left, rect.top() + 17.0),
        egui::Align2::LEFT_CENTER,
        title,
        font(13.0),
        MUTED,
    );
    let value = truncated_galley(painter, value, font(15.0), TEXT, width - 68.0);
    painter.galley(
        egui::pos2(left, rect.top() + 38.0 - value.size().y / 2.0),
        value,
        TEXT,
    );
    if status.is_empty() {
        return rect;
    }
    painter.circle_filled(egui::pos2(left + 4.0, rect.top() + 72.0), 3.0, accent);
    let status = truncated_galley(painter, status, font(11.5), TEXT2, width - 80.0);
    painter.galley(
        egui::pos2(left + 12.0, rect.top() + 72.0 - status.size().y / 2.0),
        status,
        TEXT2,
    );
    rect
}

fn draw_card_icon(painter: &egui::Painter, rect: egui::Rect, icon: CardIcon, color: egui::Color32) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.2, color);
    match icon {
        CardIcon::Gpu => {
            let chip = egui::Rect::from_center_size(c, egui::vec2(14.0, 14.0));
            painter.rect_stroke(chip, 2.0, stroke, egui::StrokeKind::Inside);
            for offset in [-5.0, 0.0, 5.0] {
                painter.line_segment(
                    [
                        egui::pos2(chip.left() - 3.0, c.y + offset),
                        egui::pos2(chip.left(), c.y + offset),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        egui::pos2(chip.right(), c.y + offset),
                        egui::pos2(chip.right() + 3.0, c.y + offset),
                    ],
                    stroke,
                );
            }
        }
        CardIcon::Download => {
            painter.line_segment(
                [egui::pos2(c.x, c.y - 7.0), egui::pos2(c.x, c.y + 2.0)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 2.5)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x + 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 2.5)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 6.0, c.y + 6.0),
                    egui::pos2(c.x + 6.0, c.y + 6.0),
                ],
                stroke,
            );
        }
        CardIcon::Spark => {
            painter.line_segment(
                [egui::pos2(c.x, c.y - 7.0), egui::pos2(c.x, c.y + 7.0)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 7.0, c.y), egui::pos2(c.x + 7.0, c.y)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 4.0, c.y - 4.0),
                    egui::pos2(c.x + 4.0, c.y + 4.0),
                ],
                egui::Stroke::new(1.0, CYAN_DIM),
            );
        }
    }
}

#[derive(Clone, Copy)]
enum MaintIcon {
    Shield,
    Tool,
    Download,
    Power,
    Trash,
}

fn maintenance_card(
    ui: &mut egui::Ui,
    width: f32,
    icon: MaintIcon,
    title: &str,
    description: &str,
    enabled: bool,
    tag: Option<&str>,
) -> bool {
    let danger = matches!(icon, MaintIcon::Trash);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, 112.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, title));
    let painter = ui.painter();
    let fill = if enabled && (response.hovered() || response.has_focus()) {
        egui::Color32::from_rgb(27, 42, 60)
    } else if enabled {
        CARD_ALT
    } else {
        egui::Color32::from_rgb(15, 23, 35)
    };
    painter.rect_filled(rect, 12.0, fill);
    painter.rect_stroke(
        rect,
        12.0,
        egui::Stroke::new(
            1.0,
            if danger {
                egui::Color32::from_rgb(72, 55, 64)
            } else if enabled && response.hovered() {
                egui::Color32::from_rgb(65, 122, 150)
            } else {
                egui::Color32::from_rgb(41, 57, 78)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let icon_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(18.0, 18.0), egui::vec2(40.0, 40.0));
    painter.rect_filled(
        icon_rect,
        10.0,
        if danger {
            egui::Color32::from_rgb(51, 35, 46)
        } else {
            egui::Color32::from_rgb(26, 56, 68)
        },
    );
    let icon_color = if danger {
        RED
    } else if enabled {
        CYAN
    } else {
        MUTED
    };
    draw_maint_icon(painter, icon_rect, icon, icon_color);

    let left = rect.left() + 71.0;
    painter.text(
        egui::pos2(left, rect.top() + 25.0),
        egui::Align2::LEFT_CENTER,
        title,
        bold(13.0),
        if enabled || danger { TEXT } else { MUTED },
    );
    if let Some(tag) = tag {
        painter.text(
            egui::pos2(rect.right() - 16.0, rect.top() + 25.0),
            egui::Align2::RIGHT_CENTER,
            tag,
            font(8.5),
            MUTED,
        );
    }
    let galley = painter.layout(
        description.to_string(),
        font(11.0),
        if enabled || danger {
            egui::Color32::from_rgb(132, 150, 172)
        } else {
            MUTED
        },
        (width - 91.0).max(80.0),
    );
    painter.galley(egui::pos2(left, rect.top() + 42.0), galley, TEXT2);
    enabled && response.clicked()
}

fn draw_maint_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: MaintIcon,
    color: egui::Color32,
) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.6, color);
    match icon {
        MaintIcon::Shield => {
            painter.add(egui::Shape::closed_line(
                vec![
                    egui::pos2(c.x, c.y - 10.0),
                    egui::pos2(c.x + 8.0, c.y - 6.5),
                    egui::pos2(c.x + 7.0, c.y + 3.0),
                    egui::pos2(c.x, c.y + 10.0),
                    egui::pos2(c.x - 7.0, c.y + 3.0),
                    egui::pos2(c.x - 8.0, c.y - 6.5),
                ],
                stroke,
            ));
        }
        MaintIcon::Tool => {
            painter.line_segment(
                [
                    egui::pos2(c.x - 7.0, c.y + 8.0),
                    egui::pos2(c.x + 5.0, c.y - 4.0),
                ],
                stroke,
            );
            painter.circle_stroke(egui::pos2(c.x + 6.0, c.y - 6.0), 4.0, stroke);
            painter.line_segment(
                [
                    egui::pos2(c.x - 9.0, c.y + 5.0),
                    egui::pos2(c.x - 5.0, c.y + 9.0),
                ],
                stroke,
            );
        }
        MaintIcon::Download => {
            painter.line_segment(
                [egui::pos2(c.x, c.y - 9.0), egui::pos2(c.x, c.y + 2.0)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 2.0)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x + 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 2.0)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 7.0, c.y + 7.0),
                    egui::pos2(c.x + 7.0, c.y + 7.0),
                ],
                stroke,
            );
        }
        MaintIcon::Power => {
            draw_arc(painter, c, 8.0, -2.2, 1.1, 1.6, color);
            painter.line_segment(
                [egui::pos2(c.x, c.y - 10.0), egui::pos2(c.x, c.y + 1.0)],
                stroke,
            );
        }
        MaintIcon::Trash => {
            painter.line_segment(
                [
                    egui::pos2(c.x - 8.0, c.y - 7.0),
                    egui::pos2(c.x + 8.0, c.y - 7.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 4.0, c.y - 10.0),
                    egui::pos2(c.x + 4.0, c.y - 10.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 6.0, c.y - 5.0),
                    egui::pos2(c.x - 5.0, c.y + 9.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x + 6.0, c.y - 5.0),
                    egui::pos2(c.x + 5.0, c.y + 9.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 5.0, c.y + 9.0),
                    egui::pos2(c.x + 5.0, c.y + 9.0),
                ],
                stroke,
            );
        }
    }
}

fn game_card(
    ui: &mut egui::Ui,
    game: &GameInfo,
    status: &str,
    selected: bool,
    height: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::accesskit::Role::RadioButton,
            true,
            selected,
            format!("{}，{}", game.name, status),
        )
    });
    let painter = &ui.painter().with_clip_rect(rect);
    let fill = if selected {
        egui::Color32::from_rgb(13, 29, 43)
    } else if response.hovered() || response.has_focus() {
        egui::Color32::from_rgb(15, 26, 39)
    } else {
        CARD
    };
    painter.rect_filled(rect, 10.0, fill);
    painter.rect_stroke(
        rect,
        10.0,
        egui::Stroke::new(
            1.0,
            if selected {
                egui::Color32::from_rgb(36, 89, 105)
            } else {
                LINE
            },
        ),
        egui::StrokeKind::Inside,
    );

    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 37.0, rect.center().y),
        egui::vec2(46.0, 46.0),
    );
    painter.rect_filled(icon, 8.0, egui::Color32::from_rgb(16, 31, 48));
    painter.rect_stroke(
        icon,
        8.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(31, 57, 76)),
        egui::StrokeKind::Inside,
    );
    let mid = icon.center().y;
    let wave = [
        egui::pos2(icon.left() + 9.0, mid + 2.0),
        egui::pos2(icon.left() + 13.0, mid - 4.0),
        egui::pos2(icon.left() + 17.0, mid + 4.0),
        egui::pos2(icon.left() + 21.0, mid - 3.0),
        egui::pos2(icon.left() + 25.0, mid + 1.0),
        egui::pos2(icon.left() + 30.0, mid),
    ];
    painter.add(egui::Shape::line(
        wave.to_vec(),
        egui::Stroke::new(1.2, egui::Color32::from_rgb(74, 225, 225)),
    ));

    let left = rect.left() + 74.0;
    let install_badge = if status.starts_with("已安装 ") {
        Some((status.split(" · ").next().unwrap_or(status), GREEN, BG))
    } else if status == "外部安装" {
        Some(("外部安装", AMBER, BG))
    } else if status == "安装未完成" {
        Some(("安装未完成", RED, BG))
    } else {
        None
    };
    let badges: Vec<_> = [
        Some((
            game.compatibility.label(),
            egui::Color32::from_rgb(22, 43, 52),
            CYAN,
        )),
        install_badge,
    ]
    .into_iter()
    .flatten()
    .map(|(text, fill, color)| {
        (
            truncated_galley(painter, text, bold(11.0), color, 160.0),
            fill,
            color,
        )
    })
    .collect();
    let badge_width: f32 = badges
        .iter()
        .map(|(galley, _, _)| galley.size().x + 20.0)
        .sum();
    let name = truncated_galley(
        painter,
        &game.name,
        bold(16.0),
        TEXT,
        rect.width() - 170.0 - badge_width,
    );
    let name_rect = egui::Rect::from_min_size(
        egui::pos2(left, rect.top() + height * 0.25 - name.size().y / 2.0),
        name.size(),
    );
    painter.galley(name_rect.min, name, TEXT);
    let mut badge_left = name_rect.right() + 8.0;
    for (galley, fill, color) in badges {
        let badge = egui::Rect::from_min_size(
            egui::pos2(badge_left, name_rect.center().y - 10.0),
            egui::vec2(galley.size().x + 12.0, 20.0),
        );
        painter.rect_filled(badge, 4.0, fill);
        painter.galley(
            egui::pos2(badge.left() + 6.0, badge.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        badge_left = badge.right() + 8.0;
    }

    let api = graphics_api(game);
    let bits = match game.architecture {
        Some(64) => "64-bit",
        Some(32) => "32-bit",
        _ => "架构待确认",
    };
    for (text, y, size, color) in [
        (
            format!("{} · {} / {} · {}", game.launcher, bits, api, status),
            height * 0.55,
            12.0,
            TEXT2,
        ),
        (game.exe.display().to_string(), height * 0.80, 10.5, MUTED),
    ] {
        let galley = truncated_galley(painter, &text, font(size), color, rect.width() - 180.0);
        painter.galley(
            egui::pos2(left, rect.top() + y - galley.size().y / 2.0),
            galley,
            color,
        );
    }

    let right = rect.right() - 16.0;
    if selected {
        painter.text(
            egui::pos2(right - 22.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "已选择",
            font(11.5),
            CYAN,
        );
        painter.circle_stroke(
            egui::pos2(right - 8.0, rect.center().y),
            7.0,
            egui::Stroke::new(1.5, CYAN),
        );
        painter.circle_filled(egui::pos2(right - 8.0, rect.center().y), 3.0, CYAN);
    } else {
        painter.text(
            egui::pos2(right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "选择",
            font(11.5),
            TEXT2,
        );
    }

    response.on_hover_text("图形接口仅根据所选 EXE 的导入表识别，不代表游戏实际运行时使用的接口。未识别不等于不支持；需在游戏内验证。").clicked()
}

fn graphics_api(game: &GameInfo) -> &'static str {
    if game
        .imports
        .iter()
        .any(|name| name.eq_ignore_ascii_case("d3d12.dll"))
    {
        "D3D12"
    } else if game
        .imports
        .iter()
        .any(|name| name.eq_ignore_ascii_case("dxgi.dll"))
    {
        "DXGI"
    } else {
        "图形接口未识别"
    }
}

fn status_badge(ui: &mut egui::Ui, text: &str, fill: egui::Color32, color: egui::Color32) {
    egui::Frame::NONE
        .fill(fill)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).color(color));
        });
}

fn eyebrow(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        let (line, _) = ui.allocate_exact_size(egui::vec2(17.0, 2.0), egui::Sense::hover());
        ui.painter().rect_filled(line, 1.0, CYAN);
        ui.add_space(7.0);
        ui.label(egui::RichText::new(text).size(9.5).color(CYAN));
    });
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(9.0).color(MUTED));
    ui.add_space(7.0);
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(9.0).color(MUTED));
    ui.add_space(3.0);
}

fn settings_row(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    right: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(20.0);
        let max_width = ((ui.available_width() - 20.0) * 0.62).max(200.0);
        ui.vertical(|ui| {
            ui.set_max_width(max_width);
            ui.label(egui::RichText::new(title).font(bold(12.5)).color(TEXT));
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(description)
                        .size(10.2)
                        .color(egui::Color32::from_rgb(129, 147, 169)),
                )
                .wrap(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(20.0);
            right(ui);
        });
    });
    ui.add_space(14.0);
}

fn secondary_button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(14.0).color(TEXT2))
            .fill(egui::Color32::from_rgb(24, 38, 55))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 85, 108)))
            .corner_radius(9.0)
            .min_size(egui::vec2(width, ACTION_HEIGHT)),
    )
}

fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(12.0).color(TEXT2))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(6.0)
            .min_size(egui::vec2(0.0, 30.0)),
    )
}

fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ACTION_HEIGHT),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, text));
    let painter = ui.painter();
    let fill = if !enabled {
        egui::Color32::from_rgb(27, 42, 54)
    } else if response.hovered() || response.has_focus() {
        egui::Color32::from_rgb(177, 245, 255)
    } else {
        egui::Color32::from_rgb(145, 232, 246)
    };
    if enabled {
        painter.rect_filled(
            rect.expand(3.0),
            12.0,
            egui::Color32::from_rgba_unmultiplied(112, 226, 244, 20),
        );
    }
    painter.rect_filled(rect, 9.0, fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        bold(14.0),
        if enabled {
            egui::Color32::from_rgb(5, 22, 30)
        } else {
            MUTED
        },
    );
    enabled && response.clicked()
}

fn progress_bar(ui: &mut egui::Ui, width: f32, fraction: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 9.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.5, egui::Color32::from_rgb(37, 51, 71));
    match fraction {
        Some(fraction) => {
            let fraction = fraction.clamp(0.0, 1.0);
            if fraction > 0.0 {
                let fill = egui::Rect::from_min_size(
                    rect.min,
                    egui::vec2(rect.width() * fraction, rect.height()),
                );
                painter.rect_filled(fill, 4.5, CYAN);
            }
        }
        None => {
            let time = ui.input(|input| input.time) as f32;
            let offset = (time * 0.55).fract();
            let segment = rect.width() * 0.28;
            let x = rect.left() - segment + (rect.width() + segment) * offset;
            let fill = egui::Rect::from_min_max(
                egui::pos2(x.max(rect.left()), rect.center().y - 4.5),
                egui::pos2((x + segment).min(rect.right()), rect.center().y + 4.5),
            );
            painter.rect_filled(fill, 4.5, CYAN_DIM);
        }
    }
}

const PROGRESS_STEPS: [&str; 5] = [
    "环境与游戏预检查",
    "下载并校验固定组件",
    "解压并准备组件",
    "写入并安全部署",
    "合并 INI 并校验文件",
];

#[derive(Clone, Copy)]
enum StepState {
    Done,
    Live,
    Pending,
}

fn progress_stage_index(stage: &str) -> usize {
    match stage {
        "准备" | "预检查" => 0,
        "准备组件" | "下载主组件" | "下载固定资源" => 1,
        "解压主组件" | "解压固定资源" | "清理" => 2,
        "应用" => 3,
        "写入配置" | "验证" => 4,
        "提交" => PROGRESS_STEPS.len(),
        _ => 0,
    }
}

fn draw_progress_step(ui: &mut egui::Ui, number: usize, label: &str, state: StepState) {
    ui.horizontal(|ui| {
        let (circle, _) = ui.allocate_exact_size(egui::vec2(23.0, 23.0), egui::Sense::hover());
        let painter = ui.painter();
        match state {
            StepState::Done => {
                painter.circle_filled(circle.center(), 11.5, egui::Color32::from_rgb(29, 79, 75));
                painter.circle_stroke(
                    circle.center(),
                    11.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(61, 128, 120)),
                );
                draw_check(
                    painter,
                    circle.center(),
                    9.0,
                    egui::Color32::from_rgb(140, 237, 213),
                );
            }
            StepState::Live => {
                painter.circle_filled(circle.center(), 11.5, egui::Color32::from_rgb(36, 72, 102));
                painter.circle_stroke(
                    circle.center(),
                    11.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(109, 182, 234)),
                );
                painter.text(
                    circle.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{number}"),
                    font(10.0),
                    egui::Color32::from_rgb(190, 245, 255),
                );
            }
            StepState::Pending => {
                painter.circle_stroke(
                    circle.center(),
                    11.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(57, 80, 107)),
                );
                painter.text(
                    circle.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{number}"),
                    font(10.0),
                    MUTED,
                );
            }
        }
        ui.add_space(4.0);
        let color = match state {
            StepState::Done => egui::Color32::from_rgb(166, 232, 208),
            StepState::Live => egui::Color32::from_rgb(189, 231, 249),
            StepState::Pending => egui::Color32::from_rgb(128, 148, 170),
        };
        ui.label(egui::RichText::new(label).size(12.0).color(color));
    });
}

fn timestamp(ms: u64) -> String {
    let seconds = ms / 1000;
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let hours = remainder / 3_600;
    let minutes = (remainder % 3_600) / 60;
    format!("Unix {days}d {hours:02}:{minutes:02} UTC")
}

fn installed_key(exe: &Path) -> String {
    exe.canonicalize()
        .unwrap_or_else(|_| exe.to_path_buf())
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn is_managed_status(status: &str) -> bool {
    status.starts_with("已安装 ") || matches!(status, "外部安装" | "安装未完成")
}

// ponytail: 上游版本号格式不保证（1.13.3.3、日期串等），解析不了就按“不相等即需要更新”处理，
// 由清单发布方保证不会把旧版本当新版本；能解析时仍按数字段严格比较。
fn newer_version(latest: &str, installed: &str) -> bool {
    let latest = latest.trim().trim_start_matches(['v', 'V']);
    let installed = installed.trim().trim_start_matches(['v', 'V']);
    match (
        semver::Version::parse(latest),
        semver::Version::parse(installed),
    ) {
        (Ok(latest), Ok(installed)) => latest > installed,
        _ => match (
            numeric_version_parts(latest),
            numeric_version_parts(installed),
        ) {
            (Some(latest), Some(installed)) => latest > installed,
            _ => latest != installed,
        },
    }
}

// ponytail: 上游有 1.13.3.3 这类四段版本，semver 解析失败时按数字段比较；不做预发布排序，目录本身已拒绝预发布。
fn numeric_version_parts(value: &str) -> Option<Vec<u64>> {
    let core = value.split(['-', '+']).next()?;
    let parts: Vec<u64> = core
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() >= 3).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> (App, Sender<Event>, Receiver<Command>) {
        let (sender, commands) = channel();
        let (events, receiver) = channel();
        let game = GameInfo {
            launcher: "Steam".into(),
            name: "赛博朋克 2077 · Cyberpunk 2077: Phantom Liberty".into(),
            exe: PathBuf::from(
                "D:/SteamLibrary/steamapps/common/Cyberpunk 2077/bin/x64/Cyberpunk2077.exe",
            ),
            game_dir: PathBuf::from("D:/SteamLibrary/steamapps/common/Cyberpunk 2077/bin/x64"),
            architecture: Some(64),
            imports: vec!["dxgi.dll".into(), "d3d12.dll".into()],
            compatibility: crate::model::Compatibility::Candidate,
        };
        (
            App {
                paths: Err("UI test: no disk writes".into()),
                settings: Settings::default(),
                draft: Settings::default(),
                gpus: vec![GpuInfo {
                    name: "AMD Radeon RX 9070 XT".into(),
                    vendor_id: 0x1002,
                    device_id: 0,
                    dedicated_vram: 16 << 30,
                    luid: 0,
                    software: false,
                    mochizuki_class: crate::model::GpuClass::Rdna4,
                }],
                games: vec![game],
                scanning_games: false,
                install_statuses: HashMap::new(),
                selected_game: Some(0),
                upstream: None,
                checking_updates: false,
                upstream_error: None,
                last_update_check: None,
                app_update: None,
                screen: Screen::Select,
                page: Page::Install,
                progress: Progress::new("应用", "正在写入组件 · OptiScaler.dll", Some(0.62)),
                report: Some(MaintenanceReport {
                    title: "安装完成".into(),
                    lines: vec!["组件校验通过。仍需在游戏内验收。".into()],
                }),
                error: None,
                status: "环境检测完成".into(),
                maintenance_busy: false,
                pending_maintenance: None,
                app_update_busy: false,
                app_update_phase: None,
                app_update_progress: Progress::new("安装器更新", "准备下载", None),
                app_update_error: None,
                update_notified_version: None,
                last_maintenance: None,
                sender,
                receiver,
            },
            events,
            commands,
        )
    }

    #[test]
    fn refresh_preserves_target_by_path_and_handles_removed_games() {
        let (mut app, events, _) = fixture();
        let selected = app.games[0].clone();
        let mut other = selected.clone();
        other.exe = PathBuf::from("D:/Other/Game.exe");
        events
            .send(Event::Games(vec![other, selected.clone()]))
            .unwrap();
        app.drain_events();
        assert_eq!(app.selected_game, Some(1));
        assert_eq!(app.selected_game().unwrap().exe, selected.exe);
        events.send(Event::Games(vec![])).unwrap();
        app.drain_events();
        assert!(app.selected_game().is_none());
    }

    fn release() -> UpstreamRelease {
        UpstreamRelease {
            release_id: 1,
            tag: "v1.13.0".into(),
            published_at: String::new(),
            asset: crate::model::UpstreamAsset {
                id: 1,
                name: "OptScaler-NR-1.13.0.zip".into(),
                size: 0,
                sha256: "a".repeat(64),
                url: String::new(),
                updated_at: String::new(),
            },
            checked_at_ms: 1,
        }
    }

    fn populate_games(app: &mut App) {
        let base = app.games[0].clone();
        app.games = [
            "赛博朋克 2077",
            "黑神话：悟空",
            "霍格沃茨之遗",
            "巫师 3：狂猎",
            "荒野大镖客：救赎 2",
            "极限竞速：地平线 5",
            "最终幻想 XVI",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let mut game = base.clone();
            game.name = name.into();
            game.game_dir = PathBuf::from(format!("D:/Games/{name}"));
            game.exe = game.game_dir.join(format!("Game{index}.exe"));
            game
        })
        .collect();
        app.upstream = Some(release());
        app.install_statuses
            .insert(installed_key(&app.games[1].exe), "已安装 v1.12.2".into());
    }

    #[test]
    fn installed_games_stably_sort_first_without_changing_selected_target() {
        let (mut app, _, _) = fixture();
        populate_games(&mut app);
        let original: Vec<_> = app.games.iter().map(|game| game.exe.clone()).collect();
        app.install_statuses
            .insert(installed_key(&original[3]), "外部安装".into());
        app.install_statuses
            .insert(installed_key(&original[5]), "已安装 1.12.2".into());
        app.selected_game = Some(4);
        app.sort_games_by_install();
        assert_eq!(
            app.games.iter().map(|game| &game.exe).collect::<Vec<_>>(),
            [1, 3, 5, 0, 2, 4, 6].map(|index| &original[index])
        );
        assert_eq!(app.selected_game().unwrap().exe, original[4]);
        let once: Vec<_> = app.games.iter().map(|game| game.exe.clone()).collect();
        app.sort_games_by_install();
        assert_eq!(
            app.games.iter().map(|game| &game.exe).collect::<Vec<_>>(),
            once.iter().collect::<Vec<_>>()
        );
        app.install_statuses
            .insert(installed_key(&original[1]), "未安装".into());
        app.sort_games_by_install();
        assert_eq!(app.games[0].exe, original[3]);
        assert_eq!(app.games[1].exe, original[5]);
        assert_eq!(app.selected_game().unwrap().exe, original[4]);
        app.games.clear();
        app.sort_games_by_install();
        assert!(app.selected_game.is_none());
    }

    #[test]
    fn installed_badges_are_bright_and_keep_versions_and_update_state() {
        let (app, _, _) = fixture();
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        for width in [500.0, 900.0] {
            for height in [56.0, 84.0] {
                for (status, label, fill) in [
                    ("已安装 1.12.2", "已安装 1.12.2", GREEN),
                    ("已安装 v1.12.2 · 可更新", "已安装 v1.12.2", GREEN),
                    ("外部安装", "外部安装", AMBER),
                    ("安装未完成", "安装未完成", RED),
                ] {
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width + 16.0, 200.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.set_width(width);
                            game_card(ui, &app.games[0], status, true, height);
                        },
                    );
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label => Some(text),
                            _ => None,
                        })
                        .expect("installation badge must be visible");
                    assert_eq!(text.fallback_color, BG);
                    let text_rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                    let badge = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.fill == fill && rect.rect.contains_rect(text_rect) =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .expect("status text must sit inside the contrasting badge");
                    assert!(
                        badge.right() < width - 70.0,
                        "badge must not overlap selection action"
                    );
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    #[test]
    fn update_versions_cache_and_repeated_checks() {
        for (latest, installed, expected) in [
            ("v1.13.0", "1.12.2", true),
            ("1.10.0", "v1.9.0", true),
            ("v1.12.2", "1.12.2", false),
            ("1.9.0", "1.10.0", false),
            ("1.13.0", "1.13.0-rc.1", true),
            ("v1.13.3.3", "1.13.3", true),
            ("v1.13.3", "1.13.3.3", false),
            ("1.13.3.10", "1.13.3.9", true),
            ("1.13.3.4-hotfix", "1.13.3.3", true),
            ("latest", "unknown", true),
            ("unknown", "unknown", false),
        ] {
            assert_eq!(newer_version(latest, installed), expected);
        }
        let (mut app, events, commands) = fixture();
        app.check_updates(true);
        app.check_updates(true);
        assert!(app.checking_updates);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::CheckUpdates(_, true))
        ));
        assert!(commands.try_recv().is_err());
        events
            .send(Event::Checked {
                upstream: Ok(Box::new(release())),
                app_update: Ok(None),
                warning: None,
            })
            .unwrap();
        app.drain_events();
        assert!(!app.checking_updates);
        assert!(app.upstream_error.is_none());
        app.check_updates(true);
        events
            .send(Event::Checked {
                upstream: Err("GitHub 更新接口暂时限流".into()),
                app_update: Ok(None),
                warning: None,
            })
            .unwrap();
        app.drain_events();
        assert!(!app.checking_updates);
        assert!(app.upstream_error.is_some());
        assert_eq!(app.upstream.as_ref().unwrap().tag, "v1.13.0");
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text == "v1.13.0 · 缓存")));
        output.drop_without_applying_deltas();
        app.install_statuses
            .insert(installed_key(&app.games[0].exe), "已安装 v1.12.2".into());
        assert!(app.game_needs_update(&app.games[0]));
        app.install_statuses
            .insert(installed_key(&app.games[0].exe), "已安装 v1.13.0".into());
        assert!(!app.game_needs_update(&app.games[0]));
        assert!(app.game_has_install(&app.games[0]));
    }

    #[test]
    fn game_scan_shows_loading_until_results_and_prevents_duplicate_requests() {
        let (mut app, events, commands) = fixture();
        let base = app.games[0].clone();
        app.games.clear();
        app.selected_game = None;
        app.refresh_games();
        app.refresh_games();
        assert!(app.scanning_games);
        assert!(matches!(commands.try_recv(), Ok(Command::Refresh)));
        assert!(commands.try_recv().is_err());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        for empty in [true, false] {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(labels.contains(&"正在扫描游戏…"));
            assert!(
                !labels
                    .iter()
                    .any(|label| label.contains("没有自动发现游戏"))
            );
            if !empty {
                assert!(labels.contains(&"赛博朋克 2077"));
            }
            output.drop_without_applying_deltas();
            if empty {
                app.games.push(base.clone());
                populate_games(&mut app);
            }
        }
        events.send(Event::Games(vec![])).unwrap();
        app.drain_events();
        assert!(!app.scanning_games);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text.contains("没有自动发现游戏"))));
        output.drop_without_applying_deltas();
        app.refresh_games();
        assert!(app.scanning_games);
        let (other, _, _) = fixture();
        events.send(Event::Games(other.games)).unwrap();
        app.drain_events();
        assert!(!app.scanning_games);
        assert_eq!(app.selected_game, Some(0));
        assert_eq!(app.games.len(), 1);
        drop(commands);
        app.refresh_games();
        assert!(!app.scanning_games);
        assert!(app.error.as_deref().unwrap().contains("无法启动游戏扫描"));
    }

    #[test]
    fn update_messages_identify_installer_and_component_results_independently() {
        for component_ok in [false, true] {
            for installer_ok in [false, true] {
                let (mut app, events, _) = fixture();
                events
                    .send(Event::Checked {
                        warning: None,
                        upstream: if component_ok {
                            Ok(Box::new(release()))
                        } else {
                            Err("GitHub 限流".into())
                        },
                        app_update: if installer_ok {
                            Ok(None)
                        } else {
                            Err("签名校验失败".into())
                        },
                    })
                    .unwrap();
                app.drain_events();
                assert!(app.status.contains(if component_ok {
                    "主包组件：最新 v1.13.0"
                } else {
                    "主包组件：更新检查失败"
                }));
                assert!(app.status.contains(if installer_ok {
                    "安装器：已是最新版本"
                } else {
                    "安装器：更新检查失败"
                }));
                let check = app.last_update_check.as_deref().unwrap();
                assert!(check.contains(if component_ok {
                    "组件主包：最新 v1.13.0"
                } else {
                    "组件主包：检查失败"
                }));
                assert!(check.contains(if installer_ok {
                    "安装器：已是最新版本"
                } else {
                    "安装器：更新检查失败"
                }));
                assert_eq!(app.upstream_error.is_some(), !component_ok);
                if !installer_ok {
                    assert_eq!(
                        app.last_maintenance.as_ref().unwrap().title,
                        "安装器更新检查失败"
                    );
                    assert!(app.status.contains("安装器详情：签名校验失败"));
                }
            }
        }
    }

    #[test]
    fn incomplete_install_remains_eligible_for_uninstall() {
        assert!(is_managed_status("安装未完成"));
        assert!(is_managed_status("已安装 1.12.2"));
        assert!(is_managed_status("外部安装"));
        assert!(!is_managed_status("未安装"));
    }

    #[test]
    fn offline_catalog_is_labeled_as_cache_not_a_fresh_success() {
        let (mut app, events, _) = fixture();
        events
            .send(Event::Checked {
                upstream: Ok(Box::new(release())),
                app_update: Ok(None),
                warning: Some("network unavailable".into()),
            })
            .unwrap();
        app.drain_events();
        assert!(app.upstream_error.is_some());
        assert!(app.status.contains("主包组件与安装器"));
        assert!(app.status.contains("非实时结果"));
        assert!(!app.status.contains("已是最新版本"));
        assert_eq!(app.upstream.as_ref().unwrap().tag, "v1.13.0");
    }

    #[test]
    fn worker_reports_check_and_download_path_failures() {
        let (sender, receiver) = spawn_worker(Err("test path unavailable".into()));
        sender
            .send(Command::CheckUpdates(Settings::default(), false))
            .unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Checked {
                upstream: Err(_),
                ..
            }
        ));
    }

    #[test]
    fn five_games_fully_visible_and_update_buttons_work() {
        for size in [egui::vec2(900.0, 650.0), egui::vec2(1180.0, 760.0)] {
            let (mut app, _events, commands) = fixture();
            populate_games(&mut app);
            app.selected_game = Some(1);
            let ctx = egui::Context::default();
            install_fonts(&ctx);
            apply_style(&ctx);
            let mut output = None;
            for _ in 0..3 {
                let mut frame = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| app.render(ui),
                );
                frame.textures_delta.clear();
                output = Some(frame);
            }
            let output = output.unwrap();
            for shape in &output.shapes {
                if let egui::Shape::Rect(rect) = &shape.shape
                    && rect.stroke.width == 1.0
                    && rect.rect.width() > 400.0
                    && (56.0..=84.1).contains(&rect.rect.height())
                {
                    assert!(shape.clip_rect.bottom() <= size.y - 80.0 - CONTENT_INSET + 1.0);
                }
            }
            let title = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "OptiScaler NR" => {
                        Some(text.pos.y + text.galley.size().y * 0.5)
                    }
                    _ => None,
                })
                .unwrap();
            assert!((title - 35.0).abs() < 1.0, "header not centered: {title}");
            let button = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.rect.size() == egui::vec2(208.0, ACTION_HEIGHT) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .unwrap();
            assert!((button.center().y - (size.y - 40.0)).abs() < 1.0);
            assert!(button.bottom() <= size.y - 18.0);
            let status = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if text.galley.job.text == "环境检测通过 · 可以继续"
                            || text.galley.job.text == app.status =>
                    {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .fold(egui::Rect::NOTHING, |bounds, rect| bounds.union(rect));
            assert!(
                (status.center().y - button.center().y).abs() < 3.0,
                "status not centered: {status:?}"
            );
            let visible_rows = output
                .shapes
                .iter()
                .filter(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) => {
                        rect.stroke.width == 1.0
                            && rect.rect.width() > 400.0
                            && (56.0..=84.1).contains(&rect.rect.height())
                            && shape.clip_rect.contains_rect(rect.rect)
                    }
                    _ => false,
                })
                .count();
            assert!(
                visible_rows >= 5,
                "{size:?}: only {visible_rows} full rows: {:?}",
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.stroke.width == 1.0 && rect.rect.width() > 400.0 =>
                            Some((rect.rect, shape.clip_rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            );
            for label in ["更新到此游戏  →"] {
                let output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
                let pos = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.pos + text.galley.size() * 0.5)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing button: {label}"));
                output.drop_without_applying_deltas();
                for pressed in [true, false] {
                    ctx.run_ui(
                        egui::RawInput {
                            events: vec![
                                egui::Event::PointerMoved(pos),
                                egui::Event::PointerButton {
                                    pos,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            ..Default::default()
                        },
                        |ui| app.render(ui),
                    )
                    .drop_without_applying_deltas();
                }
                assert_eq!(app.selected_game, Some(1));
            }
            match commands.try_recv().unwrap() {
                Command::Install {
                    game,
                    upstream: Some(upstream),
                    main_package_only,
                    ..
                } => {
                    assert_eq!(game.exe, app.games[1].exe);
                    assert_eq!(upstream.tag, "v1.13.0");
                    assert!(!main_package_only);
                }
                other => panic!("unexpected command: {other:?}"),
            }
            assert!(commands.try_recv().is_err());
        }
    }

    #[test]
    fn app_update_waits_for_the_dialog_button() {
        let (mut app, events, commands) = fixture();
        events
            .send(Event::Checked {
                upstream: Err("not used".into()),
                app_update: Ok(Some(AppUpdateInfo {
                    version: "99.0.0".into(),
                    exe_url: String::new(),
                    sha256: String::new(),
                    notes: "Update notes".into(),
                })),
                warning: None,
            })
            .unwrap();
        app.drain_events();
        assert_eq!(app.app_update_phase, Some(AppUpdatePhase::Prompt));
        assert!(commands.try_recv().is_err());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 650.0),
            )),
            ..Default::default()
        };
        let first = ctx.run_ui(input(), |ui| app.render(ui));
        first.drop_without_applying_deltas();
        let output = ctx.run_ui(input(), |ui| app.render(ui));
        let has_download = output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "下载更新")
        });
        output.drop_without_applying_deltas();
        assert!(has_download);

        app.start_app_update();
        assert_eq!(app.app_update_phase, Some(AppUpdatePhase::Downloading));
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::ApplyAppUpdate(update)) if update.version == "99.0.0"
        ));

        events
            .send(Event::AppUpdateProgress(Progress::new(
                "下载程序更新",
                "已下载 50%",
                Some(0.5),
            )))
            .unwrap();
        events.send(Event::AppUpdateApplied(Ok(()))).unwrap();
        app.drain_events();
        assert_eq!(app.app_update_phase, Some(AppUpdatePhase::Ready));
        assert_eq!(app.app_update_progress.fraction, Some(0.5));
        let first = ctx.run_ui(input(), |ui| app.render(ui));
        first.drop_without_applying_deltas();
        let output = ctx.run_ui(input(), |ui| app.render(ui));
        let has_restart = output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "重新启动")
        });
        output.drop_without_applying_deltas();
        assert!(has_restart);
    }

    #[test]
    fn pages_render_at_minimum_and_default_size() {
        for size in [egui::vec2(900.0, 650.0), egui::vec2(1180.0, 760.0)] {
            for (screen, page) in [
                (Screen::Select, Page::Install),
                (Screen::Select, Page::Maintain),
                (Screen::Select, Page::Settings),
                (Screen::Installing, Page::Install),
                (Screen::Done, Page::Install),
            ] {
                let (mut app, _events, _commands) = fixture();
                app.screen = screen;
                app.page = page;
                let ctx = egui::Context::default();
                install_fonts(&ctx);
                apply_style(&ctx);
                for _ in 0..2 {
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ui| {
                            app.render(ui);
                            assert!(
                                ui.min_rect().right() <= size.x + 1.0,
                                "{screen:?}/{page:?}: {:?}",
                                ui.min_rect()
                            );
                        },
                    )
                    .drop_without_applying_deltas();
                }
            }
        }
    }

    #[test]
    fn progress_mapping_and_unicode_truncation() {
        assert_eq!(truncate("游戏🎮测试", 3), "游戏🎮…");
        assert_eq!(truncate("游戏", 2), "游戏");
        for (stage, index) in [
            ("预检查", 0),
            ("下载主组件", 1),
            ("解压固定资源", 2),
            ("清理", 2),
            ("应用", 3),
            ("验证", 4),
            ("提交", 5),
        ] {
            assert_eq!(progress_stage_index(stage), index);
        }
    }

    #[test]
    fn confirmation_layout_is_consistent_and_long_paths_keep_actions_visible() {
        for size in [egui::vec2(900.0, 650.0), egui::vec2(1180.0, 760.0)] {
            let mut expected_size = None;
            for (action, long_path) in [
                (Action::Uninstall, false),
                (Action::DisableFrameGen, false),
                (Action::Repair, false),
                (Action::Update, false),
                (Action::Update, true),
            ] {
                let (mut app, _events, commands) = fixture();
                app.page = Page::Maintain;
                let mut game = app.games[0].clone();
                if long_path {
                    game.name = "超长游戏名称与安装路径的布局验证".repeat(5);
                    game.exe = PathBuf::from(format!(
                        "D:/Games/{}/Game.exe",
                        "long-directory/".repeat(50)
                    ));
                }
                app.pending_maintenance = Some((game, action));
                let ctx = egui::Context::default();
                install_fonts(&ctx);
                apply_style(&ctx);
                ctx.global_style_mut(|style| style.animation_time = 0.0);
                let mut output = None;
                for _ in 0..4 {
                    let mut frame = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..Default::default()
                        },
                        |ui| app.render(ui),
                    );
                    frame.textures_delta.clear();
                    output = Some(frame);
                }
                let output = output.unwrap();
                let dialog = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == CARD
                                && rect.corner_radius.nw == 12
                                && rect.rect.width() > 500.0
                                && rect.rect.width() < 600.0 =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("centered dialog frame");
                assert!(
                    (dialog.center() - size.to_pos2() * 0.5).length() < 1.0,
                    "{dialog:?}"
                );
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size)
                        .shrink(24.0)
                        .contains_rect(dialog)
                );
                assert_eq!(
                    *expected_size.get_or_insert(dialog.size()),
                    dialog.size(),
                    "{:?}, long path: {long_path}",
                    app.pending_maintenance.as_ref().unwrap().1
                );
                let buttons: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.rect.size() == egui::vec2(148.0, ACTION_HEIGHT) =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(buttons.len(), 2, "{buttons:?}");
                assert_eq!(buttons[0].center().y, buttons[1].center().y);
                assert!(
                    buttons
                        .iter()
                        .all(|button| dialog.shrink(24.0).contains_rect(*button))
                );
                let warning = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.job.text.contains("请先退出游戏") =>
                        {
                            Some((
                                egui::Rect::from_min_size(text.pos, text.galley.size()),
                                shape.clip_rect,
                            ))
                        }
                        _ => None,
                    })
                    .expect("safety warning is visible even with a long path");
                assert!(warning.1.contains_rect(warning.0));
                assert!(warning.0.bottom() < buttons[0].top());
                assert!(commands.try_recv().is_err());
            }
        }
    }

    #[test]
    fn unidentified_graphics_api_is_not_a_compatibility_failure() {
        let (mut app, _, _) = fixture();
        app.games[0].imports.clear();
        assert_eq!(graphics_api(&app.games[0]), "图形接口未识别");
        assert!(app.can_install());
        app.games[0].imports.push("DXGI.DLL".into());
        assert_eq!(graphics_api(&app.games[0]), "DXGI");
        app.games[0].imports.push("D3D12.dll".into());
        assert_eq!(graphics_api(&app.games[0]), "D3D12");
    }

    #[test]
    fn maintenance_requires_confirmation_and_escape_cancels() {
        let (mut app, _events, commands) = fixture();
        app.page = Page::Maintain;
        app.pending_maintenance = Some((app.games[0].clone(), Action::Uninstall));
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1180.0, 760.0),
            )),
            ..Default::default()
        };
        for _ in 0..4 {
            ctx.run_ui(input.clone(), |ui| app.render(ui))
                .drop_without_applying_deltas();
        }
        assert!(app.pending_maintenance.is_some());
        assert!(commands.try_recv().is_err());
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        ctx.run_ui(input, |ui| app.render(ui))
            .drop_without_applying_deltas();
        assert!(app.pending_maintenance.is_none());
        assert!(commands.try_recv().is_err());

        for label in ["取消", "卸载"] {
            app.pending_maintenance = Some((app.games[0].clone(), Action::Uninstall));
            let input = egui::RawInput::default();
            let mut button = None;
            for _ in 0..4 {
                let output = ctx.run_ui(input.clone(), |ui| app.render(ui));
                button = output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                });
                output.drop_without_applying_deltas();
            }
            let pos = button.expect("confirmation button must be visible");
            for pressed in [true, false] {
                ctx.run_ui(
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| app.render(ui),
                )
                .drop_without_applying_deltas();
            }
            assert!(app.pending_maintenance.is_none());
            if label == "卸载" {
                assert!(matches!(
                    commands.try_recv(),
                    Ok(Command::Maintain {
                        action: Action::Uninstall,
                        ..
                    })
                ));
            }
            assert!(commands.try_recv().is_err());
        }
    }

    #[test]
    fn maintenance_report_is_shown_as_a_modal() {
        let (mut app, _events, _commands) = fixture();
        app.page = Page::Maintain;
        app.last_maintenance = Some(MaintenanceReport {
            title: "REFramework 安装完成".into(),
            lines: vec!["已安装 dinput8.dll".into()],
        });
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply_style(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1180.0, 760.0),
            )),
            ..Default::default()
        };
        let mut found = false;
        for _ in 0..3 {
            let output = ctx.run_ui(input(), |ui| app.render(ui));
            found = output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "REFramework 安装完成")
            });
            output.drop_without_applying_deltas();
        }
        assert!(found);
    }

    // NR_UI_SCREENSHOT_TO captures a settled native preview without touching real games.
    #[test]
    #[ignore = "opens a native preview; does not install or scan games"]
    fn preview() {
        struct Preview(App);
        impl eframe::App for Preview {
            fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
                self.0.render(ui);
                if ui.ctx().cumulative_pass_nr() == 8
                    && let Ok(path) = std::env::var("NR_UI_SCREENSHOT_TO")
                {
                    let ctx = ui.ctx().clone();
                    ui.ctx().request_screenshot(move |image| {
                        use eframe::icon_data::IconDataExt;
                        let pixels = egui::IconData {
                            width: image.width() as u32,
                            height: image.height() as u32,
                            rgba: image
                                .pixels
                                .iter()
                                .flat_map(|pixel| pixel.to_array())
                                .collect(),
                        };
                        std::fs::write(path, pixels.to_png_bytes().unwrap()).unwrap();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        ctx.request_repaint();
                    });
                }
            }
        }
        let (mut app, _events, _commands) = fixture();
        populate_games(&mut app);
        app.selected_game = Some(1);
        app.sort_games_by_install();
        match std::env::var("NR_UI_PREVIEW").as_deref() {
            Ok("loading") => {
                app.games.clear();
                app.selected_game = None;
                app.refresh_games();
            }
            Ok("rescan") => app.refresh_games(),
            Ok("empty") => {
                app.games.clear();
                app.selected_game = None;
            }
            Ok("update-error") => {
                _events
                    .send(Event::Checked {
                        warning: None,
                        upstream: Err(
                            "GitHub 更新接口暂时限流，请稍后重试；这不代表显卡或已安装的组件有问题"
                                .into(),
                        ),
                        app_update: Ok(None),
                    })
                    .unwrap();
            }
            Ok("installer-error") => {
                _events
                    .send(Event::Checked {
                        warning: None,
                        upstream: Ok(Box::new(release())),
                        app_update: Err("无法验证安装器更新清单，请稍后重试".into()),
                    })
                    .unwrap();
            }
            Ok("maintain") => app.page = Page::Maintain,
            Ok("settings") => app.page = Page::Settings,
            Ok("progress") => app.screen = Screen::Installing,
            Ok("done") => app.screen = Screen::Done,
            Ok("confirm") => {
                app.page = Page::Maintain;
                app.pending_maintenance = Some((app.games[0].clone(), Action::Uninstall));
            }
            Ok("confirm-disable") => {
                app.page = Page::Maintain;
                app.pending_maintenance = Some((app.games[0].clone(), Action::DisableFrameGen));
            }
            Ok("confirm-update") => {
                app.page = Page::Maintain;
                app.pending_maintenance = Some((app.games[0].clone(), Action::Update));
            }
            Ok("confirm-repair") => {
                app.page = Page::Maintain;
                app.pending_maintenance = Some((app.games[0].clone(), Action::Repair));
            }
            Ok("confirm-long") => {
                app.page = Page::Maintain;
                let mut game = app.games[0].clone();
                game.name = "超长游戏名称与安装路径的布局验证".repeat(5);
                game.exe = PathBuf::from(format!(
                    "D:/Games/{}/Game.exe",
                    "very-long-installation-directory/".repeat(20)
                ));
                app.pending_maintenance = Some((game, Action::Update));
            }
            Ok("unknown-api") => app.games[0].imports.clear(),
            _ => {}
        }
        let width = std::env::var("NR_UI_WIDTH")
            .ok()
            .and_then(|width| width.parse().ok())
            .unwrap_or(1180.0);
        let height = std::env::var("NR_UI_HEIGHT")
            .ok()
            .and_then(|height| height.parse().ok())
            .unwrap_or(760.0);
        eframe::run_native(
            "OptiScaler NR · UI preview",
            eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([width, height]),
                event_loop_builder: Some(Box::new(|builder| {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    builder.with_any_thread(true);
                })),
                ..Default::default()
            },
            Box::new(move |cc| {
                install_fonts(&cc.egui_ctx);
                apply_style(&cc.egui_ctx);
                cc.egui_ctx
                    .global_style_mut(|style| style.animation_time = 0.0);
                Ok(Box::new(Preview(app)))
            }),
        )
        .unwrap();
    }
}
