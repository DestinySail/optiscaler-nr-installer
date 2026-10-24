# 第三方来源与许可证

安装器源代码采用 MIT。下载或部署的游戏组件继续适用各自许可证；本文件只做来源索引，不替代上游许可证。

## 运行组件

| 组件 | 来源 | 许可证/说明 |
|---|---|---|
| OptiScaler NR 整合 | [TheAutomatic/dlss-5-amd-project](https://github.com/TheAutomatic/dlss-5-amd-project) | GPL-3.0；本项目固定主包上游 |
| OptiScaler | [optiscaler/OptiScaler](https://github.com/optiscaler/OptiScaler) | GPL-3.0 |
| OptiScaler DLSS NR 分支 | [Dagherbou/OptiScaler_DLSSNR](https://github.com/Dagherbou/OptiScaler_DLSSNR) | 许可见上游 |
| Pre-SR Multipass 分支 | [wilsjo2/OptiScaler-DLSSNR-PreSR-Multipass](https://github.com/wilsjo2/OptiScaler-DLSSNR-PreSR-Multipass) | 许可见上游 |
| 前序整合 | [MatheusGViana/dlss-5-amd-project](https://github.com/MatheusGViana/dlss-5-amd-project) | 许可见上游 |
| Mochizuki NR core | [mochizuki0323/DLSSNR-AMD](https://github.com/mochizuki0323/DLSSNR-AMD) | MIT core；组合运行时为 GPL-3.0 |
| Mochizuki 产品适配 | [MatheusFerreiraS/neural-amd-opti](https://github.com/MatheusFerreiraS/neural-amd-opti) | GPL-3.0 |
| XeFG 多帧插件 | [dashdogy/MFG-Unlocker](https://github.com/dashdogy/MFG-Unlocker) | MIT |
| Daniel 权重来源 | [danielblnc/DLSS-NR-on-AMD](https://github.com/danielblnc/DLSS-NR-on-AMD) | 许可见上游；仅维护存档 |
| AMD FidelityFX | [GPUOpen-LibrariesAndSDKs/FidelityFX-SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK) | 以随包许可证为准 |
| Intel XeSS | [intel/xess](https://github.com/intel/xess) | 以随包许可证为准 |
| REFramework nightly | [praydog/REFramework-nightly](https://github.com/praydog/REFramework-nightly) | 许可见上游；安装器只部署 `dinput8.dll` |

`nvngx_dlssnr.dll` 是 NVIDIA 专有运行库，是模型提取的源材料。安装器只部署生成好的模型和 XeFG 插件，不把 NVIDIA 源 DLL 或 Daniel 权重复制到游戏目录。NVIDIA 运行库和由它生成的模型权重是否允许再分发，需要在发布资源前确认。

Mochizuki 模型的生成关系：

- 原模型 `dlssnr.bin`：从 310.8.0 或 310.9.16 的 `WEIGHTS_HT` 资源提取；两个来源生成的原模型相同。
- INT4 `dlssnr-int4.bin`：由原模型及上游 INT4 工具生成，仅用于原模型。
- Small `dlssnr-small.bin`：从 310.9.16 的 `WEIGHTS_CC_SMALL_FP8` 资源提取，不支持 INT4。

提取与验证方法见[上游 Mochizuki 安装说明](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.13.3/docs/mochizuki.md)。安装器使用的文件路径、大小、SHA-256 和维护源材料列于 [release/extras-v1.json](../release/extras-v1.json)，不在文档中重复维护哈希表。

## 安装器直接依赖

| Crate/仓库 | 用途 | 许可证 |
|---|---|---|
| [emilk/egui](https://github.com/emilk/egui) | 桌面界面与 `eframe` | MIT OR Apache-2.0 |
| [microsoft/windows-rs](https://github.com/microsoft/windows-rs) | DXGI、互斥量和 Windows 消息框绑定 | MIT OR Apache-2.0 |
| [seanmonstar/reqwest](https://github.com/seanmonstar/reqwest) | GitHub Release 和下载 | MIT OR Apache-2.0 |
| [PolyMeilex/rfd](https://github.com/PolyMeilex/rfd) | 本地文件选择器 | MIT |
| [mitsuhiko/self-replace](https://github.com/mitsuhiko/self-replace) | Windows EXE 自替换 | Apache-2.0 |
| [jedisct1/rust-minisign-verify](https://github.com/jedisct1/rust-minisign-verify) | 更新清单验签 | MIT |
| [dtolnay/semver](https://github.com/dtolnay/semver) | 版本比较 | MIT OR Apache-2.0 |
| [serde-rs/serde](https://github.com/serde-rs/serde) | 状态序列化 | MIT OR Apache-2.0 |
| [serde-rs/json](https://github.com/serde-rs/json) | JSON 清单与记录 | MIT OR Apache-2.0 |
| [RustCrypto/hashes](https://github.com/RustCrypto/hashes) | SHA-256 | MIT OR Apache-2.0 |
| [zip-rs/zip2](https://github.com/zip-rs/zip2) | ZIP 解压 | MIT |
| [gentoo90/winreg-rs](https://github.com/gentoo90/winreg-rs) | Windows 安装记录读取 | MIT |

完整的运行时许可证和署名文件随主包及资源包发布。既有资源附带的原始来源说明保留在 [release/EXTRAS-NOTICES.md](../release/EXTRAS-NOTICES.md)，以匹配清单中的已发布文件；当前安装范围以资源清单为准。
