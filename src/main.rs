#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod assets;
mod catalog;
mod games;
mod gpu;
mod ini;
mod install;
mod model;
mod pe;
mod self_update;
mod single_instance;
mod ui;
mod updates;
mod util;

fn main() {
    let _instance = match single_instance::acquire() {
        Ok(guard) => guard,
        Err(error) => {
            show_fatal(&error);
            std::process::exit(2);
        }
    };
    if let Err(error) = run() {
        show_fatal(&error);
        std::process::exit(1);
    }
}

fn show_fatal(message: &str) {
    eprintln!("{message}");
    if std::env::args().skip(1).any(|arg| arg.starts_with("--")) {
        return;
    }
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let _ = unsafe {
        MessageBoxW(
            None,
            &windows::core::HSTRING::from(message),
            windows::core::w!("OptiScaler NR Installer"),
            MB_OK | MB_ICONERROR,
        )
    };
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--diagnose") {
        return diagnose();
    }
    if let Some(index) = args.iter().position(|arg| arg == "--install") {
        let exe = args
            .get(index + 1)
            .ok_or_else(|| "--install 后需要游戏 EXE 路径".to_string())?;
        return headless_install(std::path::PathBuf::from(exe), &args);
    }
    for (flag, action) in [
        ("--check", model::Action::Check),
        ("--repair", model::Action::Repair),
        ("--uninstall", model::Action::Uninstall),
        ("--disable-fg", model::Action::DisableFrameGen),
    ] {
        if let Some(index) = args.iter().position(|arg| arg == flag) {
            let exe = args
                .get(index + 1)
                .ok_or_else(|| format!("{flag} 后需要游戏 EXE 路径"))?;
            return headless_maintenance(std::path::PathBuf::from(exe), action, &args);
        }
    }
    let paths = model::AppPaths::discover();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 650.0])
            .with_icon(eframe::egui::IconData {
                rgba: include_bytes!("icon.rgba").to_vec(),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        model::APP_NAME,
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, paths)))),
    )
    .map_err(|error| format!("启动界面失败: {error}"))
}

fn diagnose() -> Result<(), String> {
    let paths = model::AppPaths::discover()?;
    println!("{} {}", model::APP_NAME, model::APP_VERSION);
    println!("数据目录: {}", paths.root.display());
    if let Err(error) = install::cleanup_installer_files(&paths) {
        println!("清理旧缓存失败: {error}");
    }
    for gpu in gpu::detect_gpus()? {
        println!(
            "GPU: {} | {:04X}:{:04X} | {} MiB | LUID {:016X} | {:?}",
            gpu.name,
            gpu.vendor_id,
            gpu.device_id,
            gpu.dedicated_vram / 1024 / 1024,
            gpu.luid,
            gpu.mochizuki_class
        );
    }
    for game in games::discover_games() {
        println!(
            "GAME: {} | {} | {}",
            game.name,
            game.launcher,
            game.exe.display()
        );
    }
    if let Some(path) = games::default_picker_directory() {
        println!("PICKER: {}", path.display());
    }
    let settings = util::read_json::<model::Settings>(&paths.settings).unwrap_or_default();
    match updates::check_upstream(&paths, &settings) {
        Ok(release) => println!(
            "UPSTREAM: {} | {} | {} bytes",
            release.tag, release.asset.name, release.asset.size
        ),
        Err(error) => println!("UPSTREAM ERROR: {error}"),
    }
    match self_update::check(&paths, &settings) {
        Ok(Some(update)) => println!("APP UPDATE: {}", update.version),
        Ok(None) => println!("APP UPDATE: none"),
        Err(error) => println!("APP UPDATE: {error}"),
    }
    Ok(())
}

fn headless_install(exe: std::path::PathBuf, args: &[String]) -> Result<(), String> {
    let paths = model::AppPaths::discover()?;
    let mut settings = util::read_json::<model::Settings>(&paths.settings).unwrap_or_default();
    if let Some(index) = args.iter().position(|arg| arg == "--upstream") {
        settings.local_upstream_source = Some(std::path::PathBuf::from(
            args.get(index + 1)
                .ok_or_else(|| "--upstream 后需要 ZIP 路径".to_string())?,
        ));
    }
    if let Some(index) = args.iter().position(|arg| arg == "--extras") {
        settings.local_extras_source = Some(std::path::PathBuf::from(
            args.get(index + 1)
                .ok_or_else(|| "--extras 后需要 ZIP 路径".to_string())?,
        ));
    }
    let game = games::inspect_manual(exe)?;
    let upstream = updates::check_upstream(&paths, &settings).ok();
    let mut last_stage = String::new();
    let components = assets::prepare(&paths, &settings, upstream, |progress| {
        if progress.stage != last_stage {
            println!("{}: {}", progress.stage, progress.detail);
            last_stage = progress.stage.clone();
        }
    })?;
    last_stage.clear();
    let report = install::install(&paths, &game, &components, |progress| {
        if progress.stage != last_stage {
            println!("{}: {}", progress.stage, progress.detail);
            last_stage = progress.stage.clone();
        }
    })?;
    print_report(report);
    Ok(())
}

fn headless_maintenance(
    exe: std::path::PathBuf,
    action: model::Action,
    args: &[String],
) -> Result<(), String> {
    let paths = model::AppPaths::discover()?;
    let game = games::inspect_manual(exe)?;
    let report = match action {
        model::Action::Check => install::check_install(&paths, &game)?,
        model::Action::Uninstall => install::uninstall(&paths, &game)?,
        model::Action::DisableFrameGen => install::disable_frame_gen(&paths, &game)?,
        model::Action::Repair | model::Action::Update => {
            let mut settings =
                util::read_json::<model::Settings>(&paths.settings).unwrap_or_default();
            if let Some(index) = args.iter().position(|arg| arg == "--upstream") {
                settings.local_upstream_source = Some(std::path::PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--upstream 后需要 ZIP 路径".to_string())?,
                ));
            }
            if let Some(index) = args.iter().position(|arg| arg == "--extras") {
                settings.local_extras_source = Some(std::path::PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--extras 后需要 ZIP 路径".to_string())?,
                ));
            }
            let upstream = updates::check_upstream(&paths, &settings).ok();
            let components = assets::prepare(&paths, &settings, upstream, |_| {})?;
            install::install(&paths, &game, &components, |_| {})?
        }
    };
    print_report(report);
    Ok(())
}

fn print_report(report: model::MaintenanceReport) {
    println!("{}", report.title);
    for line in report.lines {
        println!("{line}");
    }
}
