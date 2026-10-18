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
| `src/catalog.rs` | 静态更新清单验签、缓存与失败退避 |
| `src/updates.rs` | 主包元数据读取与校验下载 |
| `src/assets.rs` | 主包/固定资源缓存、解压与兼容性检查 |
| `src/install.rs` | 文件计划、旧受管文件清理、安装记录、存储清理和维护操作 |
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

## 安装失败与存储清理

安装器不执行自动回滚。开始修改游戏文件前会写入 `complete=false` 的安装记录，其中包含旧记录和新计划的文件清单；安装成功后覆盖为 `complete=true`。失败后记录仍保留，用户可从维护页卸载全部受管文件和残留临时文件，再重新安装。已有同名文件直接覆盖，预检查进度和完成报告会显示覆盖数量；安装记录随后接管这些路径，卸载时按记录删除。

启动、安装成功和卸载后都会删除组件下载缓存、解压 staging、旧事务文件、旧安装备份和安装器更新残留。数据目录只保留安装记录、设置和签名更新清单元数据。

## 安装器自更新

启动检查发现新安装器时打开更新弹窗。用户点击下载后，弹窗显示下载进度；替换完成后显示“重新启动”。重启按钮启动带 `--restart` 参数的新进程，新实例等待旧实例释放单实例锁后继续。

组件主包不属于安装器自更新。首页只显示上游最新版本，主包在部署游戏时下载。已安装游戏出现新主包时使用主包更新模式，只替换上游管理的文件，保留固定资源、模型和 INI。

启动检查始终绕过清单缓存并刷新一次；网络失败时仍回退到已验证的签名缓存。游戏运行期间的手动检查继续遵守冷却时间。

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

`.github/workflows/build.yml` 执行测试和 Release 构建。构建使用仓库变量中的公钥，因此正式渠道的 CI 产物也支持签名更新。

## 静态更新清单

客户端只读取配置发布仓库 `update-feed` 分支上的 `update-catalog.json` 和 `.minisig`，不再调用 GitHub Releases API。清单复用已有安装器签名清单字段，并独立记录 TheAutomatic 主包版本、原始下载地址、大小和 SHA-256；固定资源仍按现有独立版本管理。

自动检查命中本地缓存的有效期为 1 小时，手动检查绕过该有效期；网络或签名失败后冷却 5 分钟，包括手动检查。旧清单在每次使用前重新验签，失败刷新不会覆盖已验证内容；界面会明确说明正在使用非实时缓存。缓存仅保存一份清单，不积累历史副本。主包下载仍校验 SHA-256/大小，并只在新版校验成功后清理旧主包缓存；模型兼容性检查保持不变。

维护端脚本 [`scripts/publish-catalog.ps1`](../scripts/publish-catalog.ps1) 查询 GitHub 元数据，先验证最新稳定安装器清单的原始签名，再读取唯一上游主包的 GitHub SHA-256 digest，生成并签署聚合清单。发现新版本不等于完成游戏兼容性验证。

```powershell
.\scripts\publish-catalog.ps1 `
  -MinisignKey "C:\path\to\release.key" `
  -MinisignExe "C:\path\to\minisign.exe"
```

工作流 [`update-catalog.yml`](../.github/workflows/update-catalog.yml) 每 30 分钟、发布 Release 后或手动触发时运行。配置仓库 Secret `NR_CATALOG_SIGNING_KEY` 为本项目专用 minisign 私钥全文（非密码保护的 CI 专用密钥；必须与 `release/update-key.pub` 匹配）。工作流只在原仓库运行、不由 PR 触发，签名完成后删除临时密钥，两个清单文件以一个提交发布；生成或验签失败不会推送。元数据未变化时不产生提交。

Secret 不对访客公开，但拥有工作流修改权限的人或被攻破的工作流可能使用/窃取它。这把密钥同时具备签发安装器更新的权限，必须保护 GitHub 账号、限制仓库写权限，不能向 PR/不可信脚本暴露 Secret；疑似泄露时删除 Secret、轮换签名密钥并发布带新公钥的安装器。CI 凭证与私钥均不得提交到仓库。若不接受云端保存密钥，可关闭该工作流并在本地签署、发布清单。

定时 Actions 可能延迟，Raw CDN 也可能短暂缓存旧文件或网络失败；这避免的是客户端匿名 Releases API 配额，不保证网络永远可用。发布新版安装器后应手动运行工作流并检查静态清单，确认 `installer.version` 指向新版本。
