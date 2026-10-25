# 开发、测试与发布

本文面向维护者。行为以 `src/`、`scripts/`、`Cargo.toml` 和 `.github/workflows/build.yml` 为准。

## 环境

- Windows 10/11 x64
- Rust stable
- Visual Studio 2022 Build Tools，安装“使用 C++ 的桌面开发”工作负载
- PowerShell 7 或 Windows PowerShell 5.1

## 本地检查

```powershell
cargo test --locked
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

正式构建使用以下编译期变量。仓库默认值定义在 `src/model.rs`，更新验签公钥的默认值定义在 `src/catalog.rs`。

| 变量 | 用途 |
|---|---|
| `NR_RELEASE_REPO` | 安装器发布所在的 GitHub 仓库 |
| `NR_UPDATE_PUBKEY` | minisign 公钥的 base64 内容 |

`NR_UPDATE_PUBKEY` 只包含公钥。私钥不得进入源码、构建产物、日志或 GitHub 仓库，应由发布环境单独保存。

当前公钥在 `release/update-key.pub`。GitHub Actions 从仓库变量 `NR_UPDATE_PUBKEY` 读取同一个值。

## 固定资源

源码仓库只提交清单、脚本和说明，不提交大体积模型。固定资源以原始文件发布到 Cloudflare R2，通过 `downloads.bonvoyage.dpdns.org` 下载。

安装资源（按模型选择下载）：

- `MFGUnlocker.asi`
- `dlssnr-amd/dlssnr.bin`
- `dlssnr-amd/dlssnr-int4.bin`
- `dlssnr-amd/dlssnr-small.bin`

维护存档：

- `nvngx_dlssnr.dll`
- `dlssnr_on_amd_weights.bin`

`release/extras-v1.json` 是唯一配置来源，安装器编译时内置其中的 `files` 列表；`sources` 只供发布脚本存档源材料和许可说明。每项都有路径、R2 key、大小与 SHA-256。修改资源时更新这份清单并发布新版安装器，不再设置 `NR_EXTRAS_*` 编译变量。旧设置文件的单 ZIP `extras` 字段会被忽略；本地资源导入只接受目录，目录内须包含 XeFG 插件及所选模型。

准备包含清单全部 `files` 和 `sources` 二进制文件及原始 `EXTRAS-NOTICES.md` 的源目录，然后验证、上传和公开回读检查：

```powershell
.\scripts\publish-extras-r2.ps1 -SourceDir "C:\path\to\extras-source" -CheckOnly

.\scripts\test-publish-extras-r2.ps1

# 使用已登录的 Wrangler；没有创建桶或修改缓存规则的隐式操作。
.\scripts\publish-extras-r2.ps1 -SourceDir "C:\path\to\extras-source"
```

Wrangler 不在 PATH 中时，可通过 `-WranglerCommand @('C:\path\to\node.exe', 'C:\path\to\wrangler.js')` 指定现有命令。上传沿用当前 Wrangler 登录，不额外保存 AWS/S3 密钥。

上传器在发布前校验全部源文件；远端已有对象必须哈希一致才跳过，差异对象拒绝覆盖，只有“键不存在”才上传。发布后通过公开域名完整下载并验证大小、SHA-256 和 Range 响应。每个版本只运行一个发布者；同版本路径不能覆盖，内容变化必须使用新版本路径。

安装器下载完成后校验每个文件的大小与 SHA-256，并把 `dlssnr-int4.bin` 与主包的
`model-tools/dlssnr-int4.sha256` 比较。主包更新导致模型不兼容时，安装器会停止并等待新的资源版本。

上游主包及 REFramework 继续使用 ZIP；只有固定资源不再打包或解压。历史维护 ZIP 如需人工归档仍可执行 `scripts/package-extras.ps1 -SourceDir ...`，不参与客户端安装。对象路径与校验值直接读取 `release/extras-v1.json`，不额外维护云端配置快照或执行记录。

## 安装失败与存储清理

安装器不执行自动回滚。开始修改游戏文件前会写入 `complete=false` 的安装记录，其中包含旧记录和新计划的文件清单；安装成功后覆盖为 `complete=true`。失败后记录仍保留，用户可从维护页卸载全部受管文件和残留临时文件，再重新安装。已有同名文件直接覆盖，预检查进度和完成报告会显示覆盖数量；安装记录随后接管这些路径，卸载时按记录删除。

启动、安装成功和卸载后都会删除组件下载缓存、解压 staging、旧事务文件、旧安装备份和安装器更新残留。数据目录只保留安装记录、设置和签名更新清单元数据。

## 安装器自更新

启动检查发现新安装器时打开更新弹窗。用户点击下载后，弹窗显示下载进度；替换完成后显示“重新启动”。重启按钮启动带 `--restart` 参数的新进程，新实例等待旧实例释放单实例锁后继续。

组件主包不属于安装器自更新。首页只显示上游最新版本，主包在部署游戏时下载。已安装游戏出现新主包且模型选择不变时只更新主包，保留固定资源和用户偏好；更换模型选择时执行完整安装并调整受管模型文件。

启动检查始终绕过清单缓存并刷新一次；网络失败时仍回退到已验证的签名缓存。游戏运行期间的手动检查继续遵守冷却时间。

## REFramework 修复

维护页的“安装 REFramework”只用于 RE Engine 游戏。它从 `praydog/REFramework-nightly` 最新 Release 读取 `REFramework.zip` 的大小和 SHA-256，下载并解压后只复制根目录的 `dinput8.dll` 到游戏 EXE 所在目录。记录保存在游戏的 `reframework.json`，卸载时仅删除哈希仍匹配的受管 DLL；被后续修改的 DLL 会保留。

## 安装器发布

推荐用工作流 `.github/workflows/release-app.yml`（`workflow_dispatch`，输入版本号）。它在 Windows Runner 上跑测试、签名清单并创建 `app-v*` Release，签名私钥取自仓库 Secret `NR_CATALOG_SIGNING_KEY`，本地无需密码：

```powershell
gh workflow run release-app.yml -f version=0.2.4
```

本地手工发布仍需使用带密码的私钥（可参考下面的命令）。两种方式都要先提交版本号和 `release/notes/app-v<版本>.md`：

```powershell
.\scripts\package-app.ps1 `
  -Exe .\target\release\OptiScalerNR.exe `
  -Version 0.2.3 `
  -Repo DestinySail/optiscaler-nr-installer `
  -MinisignKey "C:\path\to\release.key" `
  -MinisignExe "C:\path\to\minisign.exe"
```

脚本生成：

```text
dist/OptiScalerNR.exe
dist/update-manifest.json
dist/update-manifest.json.minisig
dist/app-v0.2.3.sha256
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

维护端脚本 [`scripts/publish-catalog.ps1`](../scripts/publish-catalog.ps1) 查询 GitHub 元数据，先验证最新稳定安装器清单的原始签名，再读取唯一上游主包的 GitHub SHA-256 digest，生成并签署聚合清单。安装器清单校验失败会中止发布；主包或 REFramework 拉取/校验失败只降级对应组件（沿用上一版目录里的该组件，没有则省略），安装器更新照常发布，不会因为上游问题停在旧版本。主包版本号只要求不带 `alpha`/`beta`/`rc`/`preview` 等预发布标识，不按 semver 段数校验，四段或日期形式都能通过。发现新版本不等于完成游戏兼容性验证。

客户端同样按组件独立处理：验签、清单格式和安装器字段失败才算整体失败；主包字段无效时只在界面显示主包错误（继续显示已验签缓存），安装器更新提示照常弹窗。

```powershell
.\scripts\publish-catalog.ps1 `
  -MinisignKey "C:\path\to\release.key" `
  -MinisignExe "C:\path\to\minisign.exe"
```

工作流 [`update-catalog.yml`](../.github/workflows/update-catalog.yml) 每 6 小时、发布 Release 后或手动触发时运行。配置仓库 Secret `NR_CATALOG_SIGNING_KEY` 为本项目专用 minisign 私钥全文（非密码保护的 CI 专用密钥；必须与 `release/update-key.pub` 匹配）。工作流只在原仓库运行、不由 PR 触发，签名完成后删除临时密钥，两个清单文件以一个提交发布；生成或验签失败不会推送。元数据未变化时不产生提交。

仓库变量 `NR_CATALOG_UPSTREAM_TAG` 用于把目录里的上游主包固定到指定 tag（如 `v1.13.3`），给读不懂四段版本号的老客户端留过渡窗口：此时目录仍发布新版安装器，老客户端能弹窗升级。过渡结束删除该变量，目录恢复跟随上游 `releases/latest`。固定 tag 也必须是稳定发布，安装器仍会校验其大小与 SHA-256。

Secret 不对访客公开，但拥有工作流修改权限的人或被攻破的工作流可能使用/窃取它。这把密钥同时具备签发安装器更新的权限，必须保护 GitHub 账号、限制仓库写权限，不能向 PR/不可信脚本暴露 Secret；疑似泄露时删除 Secret、轮换签名密钥并发布带新公钥的安装器。CI 凭证与私钥均不得提交到仓库。若不接受云端保存密钥，可关闭该工作流并在本地签署、发布清单。

定时 Actions 可能延迟，Raw CDN 也可能短暂缓存旧文件或网络失败；这避免的是客户端匿名 Releases API 配额，不保证网络永远可用。发布新版安装器后检查对应的更新目录工作流；未触发或失败时手动运行。回读公开清单并验签，确认 `installer.version` 和 EXE 哈希指向新版本。

## 模型选择

安装页支持 310.8.0 原模型、310.9.16 Small 和双模型。`ModelSelection` 贯穿资源准备、文件计划、INI 合并与安装记录；旧记录缺少 `models` 时按原模型处理。固定清单 `release/extras-v1.json` 当前资源集为 `extras-v2`，保留原三个资源的不可变 `extras-v1` 对象路径，只新增 `extras-v2/dlssnr-amd/dlssnr-small.bin`。

Small 来源为官方 NVIDIA DLL 310.9.16，使用上游 v1.13.3 包内 `mochizuki-model.py --models small` 提取；清单固定其大小与完整 SHA-256。主包必须含 `model-tools/descriptor-small.json` 才允许选择 Small；安装原模型仍校验主包的 INT4 哈希。导入资源目录只要求所选模型及 MFGUnlocker，不要求未选模型。

命令行也可指定模型，修复不指定时沿用安装记录：

```powershell
OptiScalerNR.exe --install 'D:\Game\Game.exe' --models 310.8.0
OptiScalerNR.exe --install 'D:\Game\Game.exe' --models 310.9.16
OptiScalerNR.exe --install 'D:\Game\Game.exe' --models both
OptiScalerNR.exe --repair 'D:\Game\Game.exe'
```

`cargo test --locked` 覆盖三种模型的下载筛选、哈希失败、安装切换、主包更新、配置保留和卸载；这些检查不代表游戏内画质、性能或稳定性通过。
