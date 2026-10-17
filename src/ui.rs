use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui;

use crate::assets;
use crate::games;
use crate::gpu;
use crate::install;
use crate::model::{
    APP_NAME, APP_VERSION, Action, AppPaths, ExtrasSpec, FIXED_PROFILE, GameInfo, GpuInfo,
    MaintenanceReport, Progress, Settings, UpstreamRelease,
};
use crate::self_update::{self, AppUpdateInfo};
use crate::updates;
use crate::util::{file_contains_marker, format_bytes, read_json, write_json_atomic};

#[derive(Debug)]
enum Command {
    Refresh,
    CheckUpdates(Settings),
    Install {
        game: GameInfo,
        settings: Settings,
        upstream: Option<UpstreamRelease>,
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
    },
    Progress(Progress),
    Installed(Result<MaintenanceReport, String>),
    Maintained {
        action: Action,
        result: Result<MaintenanceReport, String>,
    },
    AppUpdateApplied(Result<PathBuf, String>),
    Status(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Select,
    Installing,
    Done,
}

pub struct App {
    paths: Result<AppPaths, String>,
    settings: Settings,
    draft: Settings,
    gpus: Vec<GpuInfo>,
    games: Vec<GameInfo>,
    install_statuses: HashMap<String, String>,
    selected_game: Option<usize>,
    upstream: Option<UpstreamRelease>,
    app_update: Option<AppUpdateInfo>,
    screen: Screen,
    progress: Progress,
    report: Option<MaintenanceReport>,
    error: Option<String>,
    status: String,
    maintenance: bool,
    maintenance_busy: bool,
    close_after_update: bool,
    last_maintenance: Option<MaintenanceReport>,
    sender: Sender<Command>,
    receiver: Receiver<Event>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Result<AppPaths, String>) -> Self {
        install_fonts(&cc.egui_ctx);
        let settings = paths
            .as_ref()
            .ok()
            .and_then(|paths| read_json::<Settings>(&paths.settings).ok())
            .unwrap_or_default();
        let (sender, receiver) = spawn_worker(paths.clone());
        let _ = sender.send(Command::Refresh);
        let _ = sender.send(Command::CheckUpdates(settings.clone()));
        Self {
            paths,
            settings: settings.clone(),
            draft: settings,
            gpus: Vec::new(),
            games: Vec::new(),
            install_statuses: HashMap::new(),
            selected_game: None,
            upstream: None,
            app_update: None,
            screen: Screen::Select,
            progress: Progress::new("准备", "正在检测设备", None),
            report: None,
            error: None,
            status: "正在检测显卡和游戏…".to_string(),
            maintenance: false,
            maintenance_busy: false,
            close_after_update: false,
            last_maintenance: None,
            sender,
            receiver,
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Event::Gpus(result) => match result {
                    Ok(gpus) => self.gpus = gpus,
                    Err(error) => self.error = Some(error),
                },
                Event::Games(games) => {
                    self.games = games;
                    if self.selected_game.is_none() && !self.games.is_empty() {
                        self.selected_game = Some(0);
                    }
                    self.refresh_installed_tags();
                    self.status = format!("已发现 {} 个候选游戏", self.games.len());
                }
                Event::Checked {
                    upstream,
                    app_update,
                } => {
                    match upstream {
                        Ok(release) => {
                            self.status = format!("上游最新版本：{}", release.tag);
                            self.upstream = Some(*release);
                        }
                        Err(error) => {
                            self.status = format!("上游检查失败：{error}");
                            if self.upstream.is_none() {
                                self.upstream = self
                                    .paths
                                    .as_ref()
                                    .ok()
                                    .and_then(updates::load_cached_upstream);
                            }
                        }
                    }
                    match app_update {
                        Ok(update) => self.app_update = update,
                        Err(error) => {
                            self.last_maintenance = Some(MaintenanceReport {
                                title: "程序自更新未启用".to_string(),
                                lines: vec![error],
                            })
                        }
                    }
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
                Event::AppUpdateApplied(result) => match result {
                    Ok(backup) => {
                        self.last_maintenance = Some(MaintenanceReport {
                            title: "程序更新已写入".to_string(),
                            lines: vec![
                                "关闭本窗口后重新打开即可使用新版本。".to_string(),
                                format!("上一版安装器：{}", backup.display()),
                            ],
                        });
                        self.close_after_update = true;
                    }
                    Err(error) => {
                        self.last_maintenance = Some(MaintenanceReport {
                            title: "程序更新失败".to_string(),
                            lines: vec![error],
                        });
                    }
                },
                Event::Status(status) => self.status = status,
            }
        }
        if self.close_after_update {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
            let status = if let Some(tag) = install::installed_tag(paths, game) {
                format!("已安装 {tag}")
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
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP", &["zip"])
            .pick_file()
        {
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
        let _ = self.sender.send(Command::Install {
            game,
            settings: self.settings.clone(),
            upstream: self.upstream.clone(),
        });
    }

    fn run_maintenance(&mut self, action: Action) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        self.maintenance_busy = true;
        self.last_maintenance = None;
        let _ = self.sender.send(Command::Maintain {
            game,
            action,
            settings: self.settings.clone(),
            upstream: self.upstream.clone(),
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading(APP_NAME);
                ui.label(egui::RichText::new(format!("v{APP_VERSION}")).weak());
                if ui.button("维护与设置").clicked() {
                    self.maintenance = !self.maintenance;
                }
                if ui.button("重新扫描").clicked() {
                    self.status = "正在重新扫描…".to_string();
                    let _ = self.sender.send(Command::Refresh);
                }
            });
            ui.label(egui::RichText::new(FIXED_PROFILE).size(13.0));
            ui.add_space(8.0);
        });
    }

    fn select_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.heading("选择游戏");
            if ui.button("手动选择 EXE").clicked() {
                self.choose_game_exe();
            }
        });
        ui.label(
            egui::RichText::new("选择后直接安装。程序只使用一族固定方案，不提供后端或组件选项。")
                .weak(),
        );
        ui.add_space(8.0);

        self.gpu_panel(ui);
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .max_height(95.0)
            .show(ui, |ui| {
                if self.games.is_empty() {
                    ui.add_space(24.0);
                    ui.label("没有自动发现游戏。可“重新扫描”，或用“手动选择 EXE”。");
                }
                for index in 0..self.games.len() {
                    let game = self.games[index].clone();
                    let selected = self.selected_game == Some(index);
                    let response = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{}  ·  {}\n{}",
                                game.name,
                                game.launcher,
                                game.exe
                                    .file_name()
                                    .and_then(|value| value.to_str())
                                    .unwrap_or("游戏 EXE")
                            ))
                            .size(15.0),
                        )
                        .selected(selected)
                        .min_size(egui::vec2(ui.available_width(), 54.0)),
                    );
                    if response.clicked() {
                        self.selected_game = Some(index);
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{}  ·  {}  ·  {}  ·  {}",
                                game.compatibility.label(),
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
                                    "图形 API 待确认"
                                },
                                self.install_status(&game),
                                game.game_dir.display()
                            ))
                            .small()
                            .weak(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(game.exe.display().to_string());
                    ui.add_space(4.0);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let enabled = self.selected_game.is_some() && self.can_install();
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(egui::RichText::new("安装到此游戏").size(17.0))
                        .min_size(egui::vec2(220.0, 40.0)),
                )
                .clicked()
            {
                self.install_selected();
            }
            if !self.can_install() {
                ui.colored_label(
                    egui::Color32::from_rgb(217, 154, 69),
                    "需要 AMD RDNA4（RX 9070 / 9060，Navi 48/44）",
                );
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&self.status).small().weak());
    }

    fn gpu_panel(&self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("显卡检测");
                if self.gpus.is_empty() {
                    ui.spinner();
                }
            });
            if self.gpus.is_empty() {
                ui.label("正在查询 DXGI…");
            }
            let mut seen = std::collections::BTreeMap::new();
            for gpu in &self.gpus {
                let key = (gpu.software, gpu.name.clone(), gpu.vendor_id, gpu.device_id);
                seen.entry(key)
                    .and_modify(|count| *count += 1)
                    .or_insert(1usize);
            }
            for ((software, name, vendor, device), count) in seen {
                if software {
                    continue;
                }
                let gpu = self
                    .gpus
                    .iter()
                    .find(|gpu| {
                        gpu.name == name && gpu.vendor_id == vendor && gpu.device_id == device
                    })
                    .expect("grouped GPU exists");
                let supported = gpu.mochizuki_class.supports_fixed_profile();
                let color = if supported {
                    egui::Color32::from_rgb(93, 180, 127)
                } else {
                    egui::Color32::from_rgb(196, 126, 112)
                };
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(color, if supported { "符合" } else { "不符合" });
                    ui.label(format!(
                        "{}{} · {:04X}:{:04X} · {}",
                        gpu.name,
                        if count > 1 {
                            format!(" ×{count}")
                        } else {
                            String::new()
                        },
                        vendor,
                        device,
                        format_bytes(gpu.dedicated_vram)
                    ));
                });
            }
        });
    }

    fn progress_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(36.0);
        ui.heading("安装固定方案");
        ui.label(&self.progress.stage);
        ui.add_space(12.0);
        let fraction = self.progress.fraction.unwrap_or(0.0);
        ui.add(
            egui::ProgressBar::new(fraction)
                .animate(self.progress.fraction.is_none())
                .show_percentage()
                .desired_width(ui.available_width()),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new(&self.progress.detail).weak());
        ui.add_space(24.0);
        if let Some(game) = self.selected_game() {
            ui.label(format!("目标：{}", game.name));
            ui.label(
                egui::RichText::new(game.exe.display().to_string())
                    .small()
                    .weak(),
            );
        }
        ui.add_space(24.0);
        ui.label("安装期间不要启动游戏，也不要关闭程序。");
    }

    fn done_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(28.0);
        ui.heading(
            self.report
                .as_ref()
                .map(|report| report.title.as_str())
                .unwrap_or("安装完成"),
        );
        if let Some(report) = self.report.as_ref() {
            for line in &report.lines {
                ui.label(line);
            }
        }
        ui.add_space(18.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("进入游戏后");
            ui.label("1. 在游戏画面设置中开启 DLSS 帧生成。");
            ui.label("2. 按 Ins 打开 OptiScaler，确认 NR runtime 为 mochizuki、FG 输出为 XeFG。");
            ui.label("3. 首次进入场景后退出游戏再启动一次，让 HighResMV 检测结果生效。");
        });
        ui.add_space(16.0);
        if ui
            .add(egui::Button::new("返回游戏列表").min_size(egui::vec2(180.0, 38.0)))
            .clicked()
        {
            self.screen = Screen::Select;
            self.report = None;
        }
    }

    fn maintenance_window(&mut self, ctx: &egui::Context) {
        if !self.maintenance {
            return;
        }
        let mut open = self.maintenance;
        egui::Window::new("维护与设置")
            .open(&mut open)
            .default_width(560.0)
            .min_width(460.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .show(ui, |ui| {
                        ui.strong("更新状态");
                        if let Some(upstream) = &self.upstream {
                            ui.label(format!(
                                "上游 {} · {} · 检查于 {}",
                                upstream.tag,
                                upstream.asset.name,
                                timestamp(upstream.checked_at_ms)
                            ));
                        } else {
                            ui.label("尚未取得上游版本");
                        }
                        if let Some(update) = &self.app_update {
                            ui.add_space(6.0);
                            ui.label(format!("安装器新版本：{}", update.version));
                            if !update.notes.is_empty() {
                                ui.label(egui::RichText::new(&update.notes).small());
                            }
                            if ui.button("下载并更新程序").clicked() {
                                let _ = self.sender.send(Command::ApplyAppUpdate(update.clone()));
                            }
                        }
                        if ui.button("立即检查更新").clicked() {
                            self.status = "正在检查更新…".to_string();
                            let _ = self
                                .sender
                                .send(Command::CheckUpdates(self.settings.clone()));
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        ui.strong("安装器发布仓库");
                        ui.horizontal(|ui| {
                            ui.label("GitHub");
                            ui.text_edit_singleline(&mut self.draft.release_repo);
                        });
                        ui.add_space(8.0);
                        ui.strong("固定资源");
                        ui.label("extra 文件作为独立 Release 附件提供，程序只读取固定版本和哈希。");
                        ui.horizontal(|ui| {
                            ui.label("版本");
                            if let Some(extras) = self.draft.extras.as_mut() {
                                ui.text_edit_singleline(&mut extras.version);
                            } else {
                                if ui.button("添加固定资源").clicked() {
                                    self.draft.extras = Some(ExtrasSpec {
                                        version: "extras-v1".to_string(),
                                        url: String::new(),
                                        sha256: String::new(),
                                    });
                                }
                            }
                        });
                        if let Some(extras) = self.draft.extras.as_mut() {
                            ui.horizontal(|ui| {
                                ui.label("URL");
                                ui.text_edit_singleline(&mut extras.url);
                            });
                            ui.horizontal(|ui| {
                                ui.label("SHA-256");
                                ui.text_edit_singleline(&mut extras.sha256);
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.button("导入本地主包 ZIP").clicked() {
                                self.choose_upstream_source();
                            }
                            if ui.button("导入本地 extra ZIP").clicked() {
                                self.choose_extras_source();
                            }
                        });
                        if let Some(path) = &self.draft.local_upstream_source {
                            ui.label(
                                egui::RichText::new(format!("主包：{}", path.display()))
                                    .small()
                                    .weak(),
                            );
                        }
                        if let Some(path) = &self.draft.local_extras_source {
                            ui.label(
                                egui::RichText::new(format!("extra：{}", path.display()))
                                    .small()
                                    .weak(),
                            );
                        }
                        if ui.button("保存设置").clicked() {
                            self.save_settings();
                        }

                        ui.add_space(12.0);
                        ui.separator();
                        ui.strong("游戏维护");
                        if self.selected_game.is_some() {
                            ui.horizontal_wrapped(|ui| {
                                for action in [
                                    Action::Check,
                                    Action::Repair,
                                    Action::Update,
                                    Action::DisableFrameGen,
                                    Action::Uninstall,
                                ] {
                                    if ui
                                        .add_enabled(
                                            !self.maintenance_busy,
                                            egui::Button::new(action.label()),
                                        )
                                        .clicked()
                                    {
                                        self.run_maintenance(action);
                                    }
                                }
                            });
                        } else {
                            ui.label("先在主窗口选择一个游戏。");
                        }
                        if self.maintenance_busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("正在执行维护操作…");
                            });
                        }
                        if let Some(report) = &self.last_maintenance {
                            ui.add_space(8.0);
                            ui.group(|ui| {
                                ui.strong(&report.title);
                                for line in &report.lines {
                                    ui.label(line);
                                }
                            });
                        }
                    });
            });
        self.maintenance = open;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        self.top_bar(ui);
        if let Some(error) = self.error.clone() {
            egui::Panel::top("error").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(220, 100, 90), error);
                    if ui.button("关闭").clicked() {
                        self.error = None;
                    }
                });
            });
        }
        egui::CentralPanel::default().show(ui, |ui| match self.screen {
            Screen::Select => self.select_screen(ui),
            Screen::Installing => self.progress_screen(ui),
            Screen::Done => self.done_screen(ui),
        });
        self.maintenance_window(&ctx);
        ctx.request_repaint_after(Duration::from_millis(120));
    }
}

fn spawn_worker(paths: Result<AppPaths, String>) -> (Sender<Command>, Receiver<Event>) {
    let (command_tx, command_rx) = channel::<Command>();
    let (event_tx, event_rx) = channel::<Event>();
    std::thread::spawn(move || {
        if let Ok(paths) = paths.as_ref() {
            for message in install::recover_transactions(paths) {
                let _ = event_tx.send(Event::Status(message));
            }
        }
        for command in command_rx {
            match command {
                Command::Refresh => {
                    let gpu_result = gpu::detect_gpus();
                    let game_result = games::discover_games();
                    let _ = event_tx.send(Event::Gpus(gpu_result));
                    let _ = event_tx.send(Event::Games(game_result));
                }
                Command::CheckUpdates(settings) => {
                    let paths = match &paths {
                        Ok(paths) => paths,
                        Err(error) => {
                            let _ = event_tx.send(Event::Status(error.clone()));
                            continue;
                        }
                    };
                    let upstream = updates::check_upstream(paths);
                    let app_update = self_update::check(&settings);
                    let _ = event_tx.send(Event::Checked {
                        upstream: upstream.map(Box::new),
                        app_update,
                    });
                }
                Command::Install {
                    game,
                    settings,
                    upstream,
                } => {
                    let result = match &paths {
                        Ok(paths) => run_install(paths, &settings, &game, upstream, &event_tx),
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
                        Action::Repair | Action::Update => {
                            run_install(paths, &settings, &game, upstream, &event_tx)
                        }
                        Action::Check => install::check_install(paths, &game),
                        Action::Uninstall => install::uninstall(paths, &game),
                        Action::DisableFrameGen => install::disable_frame_gen(paths, &game),
                    };
                    let _ = event_tx.send(Event::Maintained { action, result });
                }
                Command::ApplyAppUpdate(update) => {
                    let result = match &paths {
                        Ok(paths) => self_update::apply(paths, &update),
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
    event_tx: &Sender<Event>,
) -> Result<MaintenanceReport, String> {
    let components = assets::prepare(paths, settings, upstream, |progress| {
        let _ = event_tx.send(Event::Progress(progress));
    })?;
    let result = install::install(paths, game, &components, |progress| {
        let _ = event_tx.send(Event::Progress(progress));
    });
    let _ = install::clear_staging(paths);
    result
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyhbd.ttc",
        "C:/Windows/Fonts/simhei.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_string(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .insert(0, "cjk".to_string());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
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
