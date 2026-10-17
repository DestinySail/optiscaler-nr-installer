# OptiScaler NR Installer

Rust / egui 编写的 Windows x64 安装器，只安装一套固定组合：

**政宗 SAMA OptiScaler NR + Mochizuki 后端 + Intel XeFG（README 方案 1）**

正常流程是：选择游戏 → 显示真实安装进度 → 完成。显卡检测、组件下载、校验、备份、INI 合并和安装记录都在后台完成。

## 当前实现

- DXGI 枚举全部显卡，按 Vendor ID、Device ID 和名称识别 RDNA4（RX 9070 / 9060，Navi 48/44）。不满足时禁止新安装，但维护入口仍可用。
- 扫描 Steam 库、Epic 清单、鸣潮常见目录，并支持手动选择 64 位游戏 EXE。
- 只把 `TheAutomatic/dlss-5-amd-project` 作为外部组件上游，读取其 latest stable Release 和 GitHub asset SHA-256。
- `extra` 不进入普通 Git 历史，作为自己的版本化 Release 附件发布。客户端按固定 URL、版本和 SHA-256 使用，不逐仓库检查插件或模型上游。
- 下载支持 `.part` 断点续传、三次重试、大小与 SHA-256 校验。
- ZIP 解压拒绝路径穿越、符号链接、大小写冲突和超限内容。
- 安装前备份、事务日志、失败恢复、文件哈希验证、INI 分段定点合并。
- 维护入口提供检查、修复、更新、回滚、关闭插帧和卸载。
- 程序更新使用自有 `app-v*` Release、签名清单和 `self-replace`；普通组件包 Release 不会被误认成程序更新。

## 构建

需要 Rust stable、Visual Studio 2022 Build Tools 的“使用 C++ 的桌面开发”组件。

```powershell
cargo test
cargo build --release
```

产物：`target\release\OptiScalerNR.exe`。

发布构建可以设置以下编译期变量：

```powershell
$env:NR_RELEASE_REPO = "DestinySail/optiscaler-nr-installer"
$env:NR_EXTRAS_VERSION = "extras-v1"
$env:NR_EXTRAS_URL = "https://github.com/DestinySail/optiscaler-nr-installer/releases/download/extras-v1/extras-v1.zip"
$env:NR_EXTRAS_SHA256 = "<release asset SHA-256>"
$env:NR_UPDATE_PUBKEY = "<minisign public key base64>"
cargo build --release
```

没有 `NR_UPDATE_PUBKEY` 的开发构建仍可正常安装组件，只会关闭程序自更新。

## 发布 `extra`

必装资源源文件约 210 MiB，压缩包约 150 MiB，不能作为普通 Git 文件提交。先在自己的仓库创建 Release `extras-v1`，然后运行：

```powershell
.\scripts\package-extras.ps1 -SourceDir "C:\path\to\payload\extra"
```

脚本生成：

```text
dist/extras-v1.zip
dist/extras-v1.sha256
dist/SHA256SUMS.txt
```

把 ZIP 和校验文件上传到 `extras-v1` Release。ZIP 根目录保持 `MFGUnlocker.asi`、`dlssnr-amd\dlssnr.bin`、`dlssnr-amd\dlssnr-int4.bin` 的相对布局。发行前确认模型与二进制的再分发许可，并保留原项目署名。

来源清单见 [release/EXTRAS-NOTICES.md](release/EXTRAS-NOTICES.md)。

不参与安装的 NVIDIA 源 DLL 与 Daniel 权重可单独发布到 `extras-maintenance-v1`：

```powershell
.\scripts\package-extras.ps1 -Kind maintenance -SourceDir "C:\path\to\maintenance-source"
```

## 应用发布与自更新

1. 创建 `app-v0.2.0` 等独立 tag，上传 `OptiScalerNR.exe`、`update-manifest.json` 和 `update-manifest.json.minisig`。
2. 清单格式：

```json
{
  "schema": 1,
  "version": "0.2.0",
  "platform": "windows-x64",
  "exe_url": "https://github.com/DestinySail/optiscaler-nr-installer/releases/download/app-v0.2.0/OptiScalerNR.exe",
  "sha256": "<64 hex characters>",
  "notes": "Release notes"
}
```

3. 用 minisign 私钥签署清单。私钥只放发布环境，不能进入源码或 EXE。
4. 应用只接受 `app-v*`、非 prerelease、版本号高于当前且清单签名有效的更新。

可在构建后生成清单并可选直接签名：

```powershell
.\scripts\package-app.ps1 `
  -Exe .\target\release\OptiScalerNR.exe `
  -Version 0.2.0 `
  -MinisignKey "$env:USERPROFILE\.minisign\installer.key" `
  -MinisignExe "C:\path\to\minisign.exe"
```

## 本地验证

```powershell
# 只读检查显卡、游戏和上游
.\target\release\OptiScalerNR.exe --diagnose

# 使用本地主包和 extra ZIP 做安装验证
.\target\release\OptiScalerNR.exe --install "D:\Game\Game.exe" --upstream ".\OptScaler-NR-1.12.2.zip" --extras ".\extras-v1.zip"

# 维护操作
.\target\release\OptiScalerNR.exe --check "D:\Game\Game.exe"
.\target\release\OptiScalerNR.exe --uninstall "D:\Game\Game.exe"
```

安装记录和缓存位于 `%LOCALAPPDATA%\OptiScalerNR`。游戏目录中的 `NRInstaller` 前缀临时文件只用于同目录替换；安装成功后事务日志会删除。

## 边界

- 当前固定方案面向 RDNA4。其他 AMD 显卡不会因为“检测到 AMD”而被放行。
- “发现游戏”“允许安装”“游戏内实测通过”是三种不同状态。自动发现的候选仍需在实际游戏中验证 NR 和 XeFG。
- 首次运行仍需在游戏内开启 DLSS 帧生成；首次进入场景后应重启游戏，使 XeFG `HighResMV` 设置生效。
- 联网游戏的反作弊提示必须立即停用本方案，程序不提供绕过。
