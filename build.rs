fn main() {
    println!("cargo:rerun-if-changed=assets/OptiScalerNR.ico");
    println!("cargo:rerun-if-changed=assets/icon.rc");
    let target = std::env::var("TARGET").unwrap();
    if !target.ends_with("windows-msvc") {
        return;
    }
    let kits = std::path::PathBuf::from(
        std::env::var_os("ProgramFiles(x86)").expect("ProgramFiles(x86) is required"),
    )
    .join("Windows Kits/10/bin");
    let host = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };
    let mut versions: Vec<_> = std::fs::read_dir(kits)
        .expect("Windows SDK is required")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    versions.sort_by_key(|path| {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    });
    let rc = versions
        .into_iter()
        .rev()
        .map(|path| path.join(host).join("rc.exe"))
        .find(|path| path.is_file())
        .expect("Windows SDK rc.exe is required");
    let resource = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("icon.res");
    let status = std::process::Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg("assets/icon.rc")
        .status()
        .expect("failed to run resource compiler");
    assert!(status.success(), "failed to compile application icon");
    println!(
        "cargo:rustc-link-arg-bin=OptiScalerNR={}",
        resource.display()
    );
}
