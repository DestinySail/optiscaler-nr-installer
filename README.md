# OptiScaler NR Installer

[![Latest release](https://img.shields.io/github/v/release/DestinySail/optiscaler-nr-installer?label=release)](https://github.com/DestinySail/optiscaler-nr-installer/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

用于 Windows x64 的 Rust 安装器，安装一套固定组合：

**OptiScaler NR + Mochizuki 后端 + Intel XeFG**

程序负责检测硬件和游戏、下载并校验组件、清理旧注入、合并配置以及后续检查、修复、更新和卸载。正常流程只有“选择游戏 → 安装进度 → 完成”。

> 当前面向 AMD RDNA4（RX 9070 / RX 9060，Navi 48/44）。其他显卡可以启动程序，但不会通过新安装条件。

## 功能

- DXGI 显卡检测，显示名称、PCI 厂商/设备 ID 和显存。
- 扫描 Steam、Epic、Windows 卸载记录与常见游戏目录，也支持手动选择游戏 EXE。
- 只跟踪 [TheAutomatic/dlss-5-amd-project](https://github.com/TheAutomatic/dlss-5-amd-project) 的稳定 Release。
- 组件主包在部署游戏时下载；首页只显示上游最新版本，不单独提供下载按钮。
- 已安装游戏有新主包时，“更新主包到此游戏”只替换主包文件；固定资源和用户配置保持不变。
- 下载主包和固定资源包，支持断点续传、重试、大小与 SHA-256 校验。
- 维护页可安装最新 REFramework nightly 的 `dinput8.dll`，修复 PRAGMATA 等 RE Engine 游戏启动后自动关闭。
- 更新时清理旧版受管文件、校验文件哈希并安全合并 INI；安装失败保留未完成记录，可卸载后重装。
- 检查、修复、更新、关闭插帧和卸载。
- 单实例运行，避免两个安装任务同时修改游戏。
- 启动时检查安装器自身的新版本；发现更新会弹窗提示，在弹窗内显示下载进度，完成后可重新启动。
- 初装默认 2X XeFG；后续维护保留用户调整的帧生成开关和插帧倍率。

## 系统要求

- Windows 10 或 Windows 11，x64。
- AMD RDNA4 独立显卡。
- 游戏为 64 位 D3D12 应用，并支持本固定方案所需的 DLSSG/Streamline 通道。
- 首次使用时需要能够访问 GitHub Releases 和 Cloudflare 下载域名 `downloads.bonvoyage.dpdns.org`。

## 快速开始

1. 从 [Releases](https://github.com/DestinySail/optiscaler-nr-installer/releases/latest) 下载 `OptiScalerNR.exe`。
2. 启动程序，自动发现的游戏会显示在列表中；未发现时可手动选择游戏 EXE。
3. 选择游戏并点击“安装到此游戏”，等待所有阶段完成。
4. 启动游戏，在画面设置中开启 DLSS 帧生成。
5. 按 `Insert` 打开 OptiScaler，确认 NR 后端为 `mochizuki`、帧生成输出为 `XeFG`。
6. 首次进入游戏场景后退出并重新启动游戏，使 XeFG 的 `HighResMV` 检测结果生效。

安装前请完全退出游戏。若游戏正在运行、目录不可写或组件哈希不符，安装器会停止。已存在的同名文件会直接覆盖，安装进度和完成报告会显示覆盖数量；卸载只删除安装记录中的文件，其他玩家文件保持原样。

## 维护

“维护与设置”中提供：

| 操作 | 行为 |
|---|---|
| 检查 | 验证受管文件、固定配置和代理状态，不修改文件 |
| 修复 | 补齐缺失或损坏的受管文件，保留用户配置 |
| 更新 | 移除当前注入，再安装固定方案的最新兼容主包和资源 |
| 安装 REFramework | 下载最新 nightly 的 `REFramework.zip`，只部署 `dinput8.dll` |
| 关闭插帧 | 移出 XeFG 插件并关闭帧生成，用于排障 |
| 卸载 | 删除本项目注入的文件，不保留旧版本备份 |

安装记录和签名更新元数据位于 `%LOCALAPPDATA%\OptiScalerNR`。组件下载包、解压目录、旧事务文件和旧安装器文件会在启动、安装完成或卸载后清理。安装器不备份被覆盖的同名文件，自动回滚已取消；更新和卸载只清理本项目记录的文件。

## 兼容性与风险

- 鸣潮的游戏发现和安装路径已在本机验证；本轮没有启动游戏做 NR/XeFG 画面验收。
- 其他自动发现的游戏会显示为“候选”，在实际游戏内验证前不能视为已支持。
- “发现游戏”“允许安装”“游戏内实测通过”是三种不同状态。
- 联网游戏的反作弊提示必须立即停用本方案；本项目不提供绕过。
- 固定资源从 Cloudflare 逐文件下载，并按安装器内置清单校验大小与 SHA-256，无需解压资源包。模型和二进制继续适用各自上游许可证，详见[来源与第三方组件](docs/THIRD_PARTY.md)和[资源来源说明](https://downloads.bonvoyage.dpdns.org/optiscaler-nr/extras-v1/EXTRAS-NOTICES.md)。

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
