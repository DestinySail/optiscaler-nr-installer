# 开发、测试与发布

本文面向维护者。行为以 `src/`、`scripts/`、`Cargo.toml` 和 `.github/workflows/build.yml` 为准。

## 环境

- Windows 10/11 x64
- Rust stable
- Visual Studio 2022 Build Tools，安装“使用 C++ 的桌面开发”工作负载
- PowerShell 7 或 Windows PowerShell 5.1

## 本地检查

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release --locked
```

测试覆盖：

- RDNA4 显卡识别
- INI 分段定点修改、重复键和用户偏好保留
- ZIP 路径与 PE 解析边界
- minisign 更新清单验签与篡改拒绝
- 单实例锁
- OptiScaler 代理标记识别

## 代码结构

| 路径 | 职责 |
|---|---|
| `src/gpu.rs` | DXGI 显卡发现与 RDNA4 识别 |
| `src/games.rs` | Steam、Epic、Windows 卸载记录和手动游戏发现 |
| `src/updates.rs` | TheAutomatic Release 查询与下载 |
| `src/assets.rs` | 主包/固定资源缓存、解压与兼容性检查 |
| `src/install.rs` | 文件计划、备份、事务、回滚、维护操作 |
| `src/ini.rs` | 固定方案配置合并与只读检查 |
| `src/self_update.rs` | 安装器签名更新清单 |
| `src/ui.rs` | 三步界面和维护入口 |

## 构建期配置

正式构建使用以下编译期变量。默认值定义在 `src/model.rs`。

| 变量 | 用途 |
|---|---|
| `NR_RELEASE_REPO` | 安装器和固定资源所在的 GitHub 仓库 |
| `NR_UPDATE_PUBKEY` | minisign 公钥的 base64 内容 |
| `NR_EXTRAS_VERSION` | 固定资源版本，例如 `extras-v1` |
| `NR_EXTRAS_URL` | 必装资源包下载地址 |
| `NR_EXTRAS_SHA256` | 必装资源包 SHA-256 |

`NR_UPDATE_PUBKEY` 只包含公钥。私钥不得进入源码、构建产物、日志或 GitHub 仓库，应由发布环境单独保存。

当前公钥在 `release/update-key.pub`。GitHub Actions 从仓库变量 `NR_UPDATE_PUBKEY` 读取同一个值。

## 资源包

源码仓库只提交清单、脚本和说明，不提交大体积模型。资源以 Release 附件发布。

必装资源：

- `MFGUnlocker.asi`
- `dlssnr-amd/dlssnr.bin`
- `dlssnr-amd/dlssnr-int4.bin`

维护存档：

- `nvngx_dlssnr.dll`
- `dlssnr_on_amd_weights.bin`

生成资源包：

```powershell
.\scripts\package-extras.ps1 -Kind runtime `
  -SourceDir "C:\path\to\extras-source" `
  -Version extras-v1

.\scripts\package-extras.ps1 -Kind maintenance `
  -SourceDir "C:\path\to\extras-source"
```

资源版本或内容变化时，需要同步更新：

- `src/model.rs` 中的默认版本、URL 和 SHA-256
- `release/extras-v1.json`
- `.github/workflows/build.yml` 中的 `NR_EXTRAS_*`
- 对应 Release 和说明

安装前会校验资源包 SHA-256，并把 `dlssnr-int4.bin` 与主包的
`model-tools/dlssnr-int4.sha256` 比较。主包更新导致模型不兼容时，安装器会停止并等待新的资源版本。

## 安装器发布

```powershell
.\scripts\package-app.ps1 `
  -Exe .\target\release\OptiScalerNR.exe `
  -Version 0.2.0 `
  -Repo DestinySail/optiscaler-nr-installer `
  -MinisignKey "C:\path\to\release.key" `
  -MinisignExe "C:\path\to\minisign.exe"
```

脚本生成：

```text
dist/OptiScalerNR.exe
dist/update-manifest.json
dist/update-manifest.json.minisig
dist/app-v0.2.0.sha256
```

安装器只接受以下更新：

- `app-v*` 标签
- 非 prerelease
- 版本高于当前版本
- 清单 minisign 签名有效
- EXE 下载地址属于配置的发布仓库
- EXE SHA-256 与签名清单一致

## CI

`.github/workflows/build.yml` 执行测试、Clippy 检查和 Release 构建。构建使用仓库变量中的公钥，因此正式渠道的 CI 产物也支持签名更新。
