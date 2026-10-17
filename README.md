# OptiScaler NR Installer

[![Latest release](https://img.shields.io/github/v/release/DestinySail/optiscaler-nr-installer?label=release)](https://github.com/DestinySail/optiscaler-nr-installer/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

用于 Windows x64 的 Rust 安装器，安装一套固定组合：

**政宗 SAMA OptiScaler NR + Mochizuki 后端 + Intel XeFG（上游 README 方案 1）**

程序负责检测硬件和游戏、下载并校验组件、清理旧注入、合并配置以及后续检查、修复、更新和卸载。正常流程只有“选择游戏 → 安装进度 → 完成”。

> 当前面向 AMD RDNA4（RX 9070 / RX 9060，Navi 48/44）。其他显卡可以启动程序，但不会通过新安装条件。

## 功能

- DXGI 显卡检测，显示名称、PCI 厂商/设备 ID 和显存。
- 扫描 Steam、Epic、Windows 卸载记录与常见游戏目录，也支持手动选择游戏 EXE。
- 只跟踪 [TheAutomatic/dlss-5-amd-project](https://github.com/TheAutomatic/dlss-5-amd-project) 的稳定 Release。
- 下载主包和固定资源包，支持断点续传、重试、大小与 SHA-256 校验。
- 安装前清理旧注入、事务日志、失败后移除注入、文件哈希验证和安全的 INI 合并。
- 检查、修复、更新、关闭插帧和卸载。
- 单实例运行，避免两个安装任务同时修改游戏。
- 使用签名更新清单检查安装器自身的新版本。
- 初装默认 2X XeFG；后续维护保留用户调整的帧生成开关和插帧倍率。

## 系统要求

- Windows 10 或 Windows 11，x64。
- AMD RDNA4 独立显卡。
- 游戏为 64 位 D3D12 应用，并支持本固定方案所需的 DLSSG/Streamline 通道。
- 首次使用时需要能够访问 GitHub Releases。

## 快速开始

1. 从 [Releases](https://github.com/DestinySail/optiscaler-nr-installer/releases/latest) 下载 `OptiScalerNR.exe`。
2. 启动程序，自动发现的游戏会显示在列表中；未发现时可手动选择游戏 EXE。
3. 选择游戏并点击“安装到此游戏”，等待所有阶段完成。
4. 启动游戏，在画面设置中开启 DLSS 帧生成。
5. 按 `Insert` 打开 OptiScaler，确认 NR 后端为 `mochizuki`、帧生成输出为 `XeFG`。
6. 首次进入游戏场景后退出并重新启动游戏，使 XeFG 的 `HighResMV` 检测结果生效。

安装前请完全退出游戏。若游戏正在运行、目录不可写、组件哈希不符或存在未知代理 DLL，安装器会停止并说明原因。

## 维护

“维护与设置”中提供：

| 操作 | 行为 |
|---|---|
| 检查 | 验证受管文件、固定配置和代理状态，不修改文件 |
| 修复 | 补齐缺失或损坏的受管文件，保留用户配置 |
| 更新 | 移除当前注入，再安装固定方案的最新兼容主包和资源 |
| 关闭插帧 | 移出 XeFG 插件并关闭帧生成，用于排障 |
| 卸载 | 删除本项目注入的文件，不保留旧版本备份 |

安装记录和缓存位于 `%LOCALAPPDATA%\OptiScalerNR`。安装器不备份游戏文件；更新和卸载只清理本项目记录的文件，游戏目录中的临时文件只用于同目录安全替换。

## 兼容性与风险

- 鸣潮的游戏发现和安装路径已在本机验证；本轮没有启动游戏做 NR/XeFG 画面验收。
- 其他自动发现的游戏会显示为“候选”，在实际游戏内验证前不能视为已支持。
- “发现游戏”“允许安装”“游戏内实测通过”是三种不同状态。
- 联网游戏的反作弊提示必须立即停用本方案；本项目不提供绕过。
- 固定资源包含的模型和二进制继续适用各自上游许可证，详见[来源与第三方组件](docs/THIRD_PARTY.md)和资源包内的 `EXTRAS-NOTICES.md`。

## 从源码构建

开发者需要 Rust stable 和 Visual Studio 2022 Build Tools 的“使用 C++ 的桌面开发”组件。

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
```

产物为 `target/release/OptiScalerNR.exe`。完整的发布构建、资源打包和签名流程见[开发与发布](docs/DEVELOPMENT.md)。

## 文档

- [文档索引](docs/README.md)
- [开发、测试与发布](docs/DEVELOPMENT.md)
- [兼容范围](docs/GAME-COMPATIBILITY.md)
- [第三方来源与许可证](docs/THIRD_PARTY.md)
- [当前实施与验证状态](docs/IMPLEMENTATION-STATUS.md)

## 参与贡献

提交问题或 Pull Request 前，请先运行 `cargo test` 和 `cargo clippy --all-targets -- -D warnings`。涉及游戏兼容性的变更需要注明 GPU、驱动、游戏版本和实际验证范围。

## 主要开源来源

- [TheAutomatic/dlss-5-amd-project](https://github.com/TheAutomatic/dlss-5-amd-project)
- [mochizuki0323/DLSSNR-AMD](https://github.com/mochizuki0323/DLSSNR-AMD)
- [MatheusFerreiraS/neural-amd-opti](https://github.com/MatheusFerreiraS/neural-amd-opti)
- [optiscaler/OptiScaler](https://github.com/optiscaler/OptiScaler)
- [dashdogy/MFG-Unlocker](https://github.com/dashdogy/MFG-Unlocker)
- [danielblnc/DLSS-NR-on-AMD](https://github.com/danielblnc/DLSS-NR-on-AMD)
- [GPUOpen-LibrariesAndSDKs/FidelityFX-SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK)
- [intel/xess](https://github.com/intel/xess)

完整来源链、安装器依赖和许可证说明见[第三方来源与许可证](docs/THIRD_PARTY.md)。

## 许可证

安装器源代码采用 [MIT License](LICENSE)。下载或部署的 OptiScaler、Mochizuki、XeFG 插件、模型和其他第三方文件不属于 MIT 授权范围，继续适用各自许可证。
