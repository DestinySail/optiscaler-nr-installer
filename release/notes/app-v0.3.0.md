# OptiScaler NR Installer 0.3.0

适配上游主包 v1.14.0（Mochizuki 0.0.8.2）。

- 神经降噪改由官方 `OptiScaler/plugins/dlssnr-amd.asi` 接管。安装器不再部署或要求 `MochizukiNrRuntime.dll`，游戏内按 `End` 打开 NR Control。
- 模型选择改为写入游戏根目录 `dlssnr-amd.ini` 的 `[Network] Model`。只装原模型时写 `CC_SILVER_AARDWOLD`，只装 Small 时写 `CC_SMALL_FP8`，两个都装时保留现有选择。首次安装使用主包自带的 RenoDX 风格模板，并同步写入 `dlssnr-amd/dlssnr-amd.default.ini`。
- 固定资源去掉 `dlssnr-int4.bin`。INT4 mixed 权重随主包内 `dlssnr-amd/int4/` 部署，不再需要单独下载或与主包做哈希比对。
- 部署前检查改为要求 `OptiScaler/plugins/dlssnr-amd.asi`；旧版主包会在部署前被拒绝。
- 更新目录工作流改为每 2 小时运行一次。

`dlssnr-amd.ini` 会被 ASI 在游戏内改写，所以不进入受管文件清单，卸载时保留；`dlssnr-amd/dlssnr-amd.default.ini` 同样保留。

验证：`cargo test --locked`、`cargo clippy --all-targets -- -D warnings`、Release 构建，以及用 v1.14.0 主包做一次本地安装、检查和卸载。
