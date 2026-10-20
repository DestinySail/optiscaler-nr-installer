# 固定资源迁移到 Cloudflare R2：直接文件下载方案

状态：迁移完成。核对日期：2026-10-20。R2 资源、下载域名、CDN 缓存规则和客户端代码已完成；0.2.2 签名安装器已正式发布，自动更新清单已验证生效。

按用户确定的方向：R2 直接存放资源文件，新版安装器逐个下载并校验，取消在线固定资源的 ZIP 打包、下载和解压。不兼容旧客户端，不增加 GitHub 回退。安装器自动更新继续负责分发新版。

## 1. Cloudflare 已验证状态

本机 Wrangler OAuth 登录已重新授权，`whoami`、账户列表、域名列表和 R2 桶列表均验证成功。使用完整的 Wrangler 4.107.0 缓存副本完成验证；未修改全局 PATH，也未修复独立的 pnpm/NVM 信任错误。

- 可用托管域名：`bonvoyage.dpdns.org`，状态 active。
- R2 已可访问，已有 `img-bed`、`lumen-shelf` 两个桶。
- 已创建专用 Standard 桶：`optiscaler-nr-assets`。
- 已绑定下载域名：`downloads.bonvoyage.dpdns.org`，Cloudflare 返回 ownership/ssl 均为 active。
- `r2.dev` 公开开发地址保持关闭。现有 Wrangler OAuth 足以创建桶、绑定域名和上传对象，但没有 Cache Rules 编辑权限。

三个安装文件、两个源材料及许可说明均已上传。六个文件都通过公开 HTTPS 完整下载、大小与 SHA-256 校验；规则启用后，六个 URL 的 Range GET 均返回 206、正确的前 1024 字节和缓存 HIT/Age。HEAD 探测仍可能返回 DYNAMIC，不能仅据此判断实际 GET 未缓存。

## 2. 分发结构

```text
新版安装器内置资源清单
  → HTTPS 下载域名
  → Cloudflare CDN
  → R2 中的三个资源文件
  → 每个文件校验通过
  → 主包与 INT4 模型兼容性检查
  → 现有安装流程
```

对象路径：

```text
optiscaler-nr/extras-v1/MFGUnlocker.asi
optiscaler-nr/extras-v1/dlssnr-amd/dlssnr.bin
optiscaler-nr/extras-v1/dlssnr-amd/dlssnr-int4.bin
optiscaler-nr/extras-v1/EXTRAS-NOTICES.md
```

下载地址前缀为 `https://downloads.bonvoyage.dpdns.org/optiscaler-nr/extras-v1/`。许可说明随资源发布供查询，不额外改变当前游戏目录的部署范围。

完整迁移还应保留原始材料，单独存放以下两个文件，用于开发维护和可复现生成；它们不进入客户端的下载列表：

```text
optiscaler-nr/sources/nvidia-310.8.0/nvngx_dlssnr.dll
optiscaler-nr/sources/daniel-v1/dlssnr_on_amd_weights.bin
```

同一份仓库资源清单可以用 `files` 记录三个安装文件、用 `sources` 记录两个原始材料的路径、大小和哈希；发布脚本上传两组，安装器只读取 `files`。不把原始 DLL 和已经生成的模型一起下载给普通安装用户。

文件内容变化就发布新版本路径；不覆盖已发布版本，不提供可变的 `latest` 地址。资源版本仍用 `extras-v1`，因为只是分发方式变化，文件内容未变。

## 3. 已核实的文件清单

本次先确认本地历史发布 ZIP 的 SHA-256 为 `ec993a9a75772c2a178d167be43a1760218001a68e6a367f1caa93be9e400533`，再流式读取其中各文件计算以下数据。历史 ZIP 仅用于确认迁移源，不进入新客户端下载流程；后续发布直接使用资源源目录。

| 文件 | 字节数 | SHA-256 |
|---|---:|---|
| `MFGUnlocker.asi` | 8929280 | `4ac118f5cbb13a556c84979c2a2d2210a756dce172a9425ff62d870694747a2e` |
| `dlssnr-amd/dlssnr.bin` | 147756560 | `2b41c888cf4155b8958c665ba64018ab0bd25c85fc71a2b6db86d0d04d1f7fbd` |
| `dlssnr-amd/dlssnr-int4.bin` | 53211392 | `22a0e2d3db1c2101a67e0d9aad066e0916afbc32cba47820f5ad24d46602319c` |
| `EXTRAS-NOTICES.md` | 1408 | `82b73554cc406c85737a77a395d8111f79b0ac2897fd42012de34e2b6777ac3f` |

必需的三个文件合计 209,897,232 字节，约 200.2 MiB；原 ZIP 约 145.4 MiB。直接文件方式省去打包与解压，但传输量增加约 38%。不为抵消这部分流量再引入自定义压缩协议。

三个文件都低于普通 Free/Pro/Business CDN 的 512 MB 单对象缓存上限。未来单文件超过上限时再评估分发方案。[官方缓存限制](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/)

### `nvngx_dlssnr.dll` 的完整用途核查

2026-10-20 已核对本项目全部相关源文件、打包脚本及上游最新稳定版 v1.12.2 的提取工具、安装脚本和运行时代码。此前“仅维护存档”的说法只描述了本安装器的部署行为，不能概括这个文件在整个上游系统中的用途。

Mochizuki 的数据链路是：

```text
nvngx_dlssnr.dll 310.8.0
  → inspect_nr.py 静态解析 PE 的 WEIGHTS_HT 资源
  → unpack_* 工具解包各网络层
  → pack_model.py 生成 dlssnr.bin
  → dlssnr-int4-weights.exe + 配套数据生成 dlssnr-int4.bin
```

`mochizuki-model.py` 不加载或执行 NVIDIA DLL，而是作为数据文件读取；它还固定检查源 DLL 的 SHA-256，只接受对应的 310.8.0。游戏中 `MochizukiBackend.cpp` 加载的是 `MochizukiNrRuntime.dll`；Mochizuki 的运行时代码读取 `dlssnr-amd/dlssnr.bin`，启用并满足 INT4 条件时读取 `dlssnr-amd/dlssnr-int4.bin`。因此，普通用户使用预生成模型时，无须再下载源 DLL。

源码证据：[提取入口](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/tools/install/mochizuki-model.py)、[PE 权重提取](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/third_party/mochizuki/linux/package/model-tools/inspect_nr.py)、[INT4 生成调用](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/tools/install/mochizuki-int4.ps1)、[Mochizuki 后端](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/OptiScaler-DLSSNR-PreSR-Multipass-main/OptiScaler/dlssnr/backend/MochizukiBackend.cpp)、[运行时模型读取](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/third_party/mochizuki/windows/src/core/nr_runtime.cpp)。

本次实际执行上游提取入口，以本地 `dist/extras-v1-source/nvngx_dlssnr.dll` 为输入，在临时目录重新生成 `dlssnr.bin`。结果为 147,756,560 字节，SHA-256 `2b41c888cf4155b8958c665ba64018ab0bd25c85fc71a2b6db86d0d04d1f7fbd`，与当前固定资源完全一致。没有覆盖原始模型或游戏文件。INT4 生成关系已核对源码，本次未重新执行 INT4 生成器。

源 DLL 自身为 165,840,496 字节，文件版本 `310,8,0,0`，SHA-256 `e16bcf15e16e13f527491cdf7845b2fe6521a738d8f7c9c721866a8496e1fc8e`，与上游提取工具要求一致。不能只替换源 DLL 为 310.9.16 就让已生成模型自动升级；还需要匹配的提取工具、模型和运行时支持。

Daniel 是另一条使用链。上游 `install-amd-presr.ps1` 在选择 Daniel 后端、缺少现成运行库或权重时会调用 `dlssnr_on_amd_setup.exe`，利用提供的 NVIDIA DLL 准备所需文件；某些条件下还会把源 DLL 复制到游戏目录。它不是本项目固定启用的 Mochizuki 后端。`dlssnr_on_amd_weights.bin` 是 Daniel 专用权重，不能直接当作 Mochizuki 的 `dlssnr.bin` 使用。[上游安装分支](https://github.com/TheAutomatic/dlss-5-amd-project/blob/v1.12.2/tools/install-amd-presr.ps1#L1100-L1135)

本地 Daniel 权重存档为 147,689,451 字节，SHA-256 `6bf8dc931ef3ccffe18c82de26ab374156e7f19539ffcf8eabaa25dca5cf15ab`。保留它是为了保存原始材料，不代表需要把 Daniel 加入当前安装方案。

额外核对了本机三个安装记录及相应游戏目录：Crimson Desert Enhanced、PRAGMATA、鸣潮均配置为 `NrBackend=mochizuki`，受管记录中都有 Mochizuki 运行时及两个模型，没有源 NVIDIA DLL 或 Daniel 权重。Crimson Desert 和 PRAGMATA 目录也确实没有源 DLL；鸣潮目录另有一个 `310.8.SF.0` 的 `nvngx_dlssnr.dll`，大小及哈希与维护源 DLL 不同，且不在安装器的受管清单中。未确认其来源，未修改它，也不能据此断言游戏当前实际加载了它。

## 4. 客户端的最小完整改动

把 `release/extras-v1.json` 从“一个 ZIP 的地址与哈希”改为“资源版本、下载地址前缀、每个文件的相对路径/大小/SHA-256”，另列 `sources` 用于原始材料存档；安装器只编译使用安装文件清单。现有 serde 即可读取，不加依赖或构建代码生成层。

这份仓库清单是唯一资源配置来源。公开桶可放清单副本便于人工核验，但客户端不从桶获取新哈希后直接信任；可信清单随签名安装器更新。

| 位置 | 实施内容 |
|---|---|
| `release/extras-v1.json` | 改成逐文件清单，填入本次核实的数据和最终域名 |
| `src/model.rs` | 定义最小逐文件清单结构并内置 JSON，删除旧单 ZIP 默认 URL/哈希和在线 `ExtrasSpec` 设置 |
| `src/assets.rs` | 按清单下载三个文件到准备目录；复用 `download_verified`；全部通过后返回现有 `extras_root`；线上固定资源不再调用解压流程 |
| `src/ui.rs` | 删除旧的单 ZIP URL/哈希输入；显示统一下载进度；本地固定资源导入改为选择资源目录 |
| `src/main.rs` | 本地 `--extras` 参数明确要求资源目录，错误信息与 GUI 一致 |
| `src/archive.rs` | 删除不再被调用的固定资源 ZIP 查找逻辑；保留上游主包仍使用的 ZIP 功能 |
| `.github/workflows/build.yml` | 删除旧 `NR_EXTRAS_VERSION/URL/SHA256` 编译环境注入，避免出现第二份配置来源 |
| `scripts/package-extras.ps1` | 取消 runtime ZIP 生成分支；维护存档用途保留，不混入此次下载迁移 |
| `scripts/publish-extras-r2.ps1` | 新增一个发布脚本，从资源目录按清单校验、上传、公开回读验证 |
| 文档、相关测试 | 同步说明和旧数据结构的调用处，留下必要回归检查 |

旧 `settings.json` 中的单 ZIP `extras` 字段不再被新代码使用；serde 默认忽略多余字段，其余设置保留。因此自动更新后不会被旧 URL 拉回 GitHub，也不需要额外的旧地址迁移框架。

本地资源目录仍执行文件存在性和模型兼容性检查。历史本地 ZIP 配置给出重新选择目录的明确信息，不把它误当成新格式。

三个文件顺序下载即可。下载失败时在准备阶段停止，不把不完整资源复制到游戏目录；只有全部大小、哈希及模型兼容性检查通过后才交给现有安装过程。下载前验证清单路径是允许的相对路径，不能越出准备目录。

复用现有 `.part` 断点续传、重试与哈希验证。不增加并行下载器或新的缓存管理层。现有启动/安装后缓存清理策略不在此次调整范围内，因此不承诺跨次启动永久复用下载文件。

## 5. 云端配置

1. 在控制台确认 `downloads.bonvoyage.dpdns.org` 没有现有用途，再创建独立的 R2 Standard 桶 `optiscaler-nr-assets`。
2. 在桶 Settings → Custom Domains 绑定该子域名，等待 HTTPS 和域名状态就绪。不要手工 CNAME 到 `r2.dev`，关闭开发公开 URL。
3. 桶只放公开资源及许可说明；凭据和安装器签名私钥不放入桶。
4. 通过 S3 API 上传逐个文件。二进制设置 `Content-Type: application/octet-stream`，许可文本设为 UTF-8 文本；版本文件设置 `Cache-Control: public,max-age=31536000,immutable`。
5. 创建 Cache Rule，精确匹配下载主机与 `/optiscaler-nr/` 路径，将 `.bin`、`.asi` 等文件设为可缓存并尊重缓存头。不能沿用“ZIP 默认可缓存”的假设；确认错误响应没有长期缓存规则。
6. 确保普通客户端能直接 GET/HEAD/Range，不被登录页面或交互验证码拦截。只按实际需要调整该下载路径的规则，不关闭全站防护。

原生 Rust 下载不需要浏览器 CORS。无需 Workers、Pages、D1、KV 或下载鉴权服务。

参考：[R2 自定义域名](https://developers.cloudflare.com/r2/buckets/public-buckets/)、[R2 CDN 缓存](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/)。

## 6. 发布流程

首版使用本地 `scripts/publish-extras-r2.ps1`，输入资源源目录，调用已经登录的 Wrangler 官方 CLI。固定资源只在实际变化时发布，不新增每六小时的同步任务，也不强制绕 GitHub 下载资源。使用前逐文件校验源目录，不因目录存在就信任内容。

沿用本机 Wrangler 的登录与自动刷新；不额外安装 AWS CLI、创建长期 S3 密钥或把 OAuth 凭据复制到仓库。`-WranglerCommand` 可指定现有 Node 与 Wrangler 的绝对路径。缓存规则由具备该域名 Cache Rules 编辑权限的凭据或已登录控制台单独配置，不提高客户端权限。

脚本执行顺序：

1. 读取项目内清单，校验源目录的三个安装资源、两个原始材料及许可说明；客户端仍只下载三个安装资源。
2. 检查目标版本对象。不存在才上传；存在则通过远端读取验证字节数和哈希，相同跳过、不同停止，不覆盖已发布版本。
3. 检查时区分 404、权限错误和网络错误；不能把所有错误视为对象不存在。保持同一版本只有一个发布者执行。
4. 使用 Wrangler 的远端对象命令上传，凭据不输出到日志；上传失败即停止。
5. 通过公开下载域名重新读取全部文件并验证大小和 SHA-256，再检测 Range 响应。
6. 全部通过才允许发布引用新资源的安装器。部分上传失败可以重跑，已验证相同的文件直接跳过；未通过验收的版本不得被客户端引用。

维护者执行入口；源目录必须包含与清单一致的许可说明，文本换行也属于哈希校验范围：

```powershell
.\scripts\publish-extras-r2.ps1 -SourceDir "C:\path\to\extras-source" -CheckOnly
.\scripts\test-publish-extras-r2.ps1
.\scripts\publish-extras-r2.ps1 -SourceDir "C:\path\to\extras-source"
```

实施时复用了已有官方工具，替代原方案中的 AWS/S3 发布方式；资源结构、哈希固定、不可覆盖和公开下载验证要求不变。

## 7. 验收、切换与故障处理

| 验收 | 通过标准 |
|---|---|
| 登录与权限 | 桶与域名已通过 API 配置并确认 active；用户已在控制台启用缓存规则，普通下载不需要管理权限 |
| 公网下载 | 三个 URL 均返回文件，大小与 SHA-256 完全匹配，无 HTML 挑战页 |
| 断点续传 | 每个文件 Range 请求返回正确的 206/Content-Range；真实中断后续传得到正确哈希 |
| CDN | 完整读取后重复访问，检查 CF-Cache-Status/Age，确认 `.bin` 和 `.asi` 真正可缓存 |
| 错误保护 | 任一文件缺失、大小错误或哈希不符，均在准备阶段失败，不进入游戏目录写入 |
| 配置 | 新版仅使用内置清单；保留旧设置文件启动也不会使用旧 ZIP 地址；其他设置保持正常 |
| 本地导入 | 合法目录可用，旧 ZIP 或缺文件给出明确错误 |
| 安装 | 在隔离游戏测试目录验证部署内容与迁移前一致，保留 INT4 与主包兼容性校验 |
| 网络 | 在用户实际常用网络完整下载一次，记录成功率及耗时，不承诺未经实测的提速 |

新增逻辑留下小型回归测试：内置清单有效；文件路径不能越界；三个文件全部通过才能完成准备；缺失/损坏文件失败；旧设置字段不会改变新版在线资源来源。发布脚本提供只校验不上传的检查方式，并验证错误源文件会退出失败。

沿用项目检查：`cargo fmt --all --check`、`cargo test --locked`、`cargo clippy --all-targets -- -D warnings` 和正式 release 构建。Range 参考：[Cloudflare Range 请求](https://developers.cloudflare.com/cache/reference/range-requests/)。

执行顺序：云资源和域名就绪 → 源文件校验并上传 → 公网下载与缓存验收 → 客户端改动及测试通过 → 发布签名新版和更新清单 → 验证自动更新后的实际下载地址。

不维护旧客户端下载、不做 GitHub 回退。遇到故障先修复对象、域名或规则；清单错误则发布修复版安装器，不能通过覆盖已发布文件来凑哈希。按用户要求，2026-10-20 已删除 GitHub 的 `extras-v1`、`extras-maintenance-v1` 资源 Release、全部附件及对应标签；普通资源和维护源材料统一保留在 R2。旧安装器应先自动更新至 0.2.2 再安装资源。

## 8. 成本和范围

R2 Standard 当前每月免费额度为 10 GB-month 存储、100 万次 A 类和 1000 万次 B 类操作，出站流量免费。超额存储 $0.015/GB-month、A 类 $4.50/百万次、B 类 $0.36/百万次；按整个账户用量计算。直接下载总请求数增加为三个资源请求，但当前资源体积与请求数量都不需要引入额外服务。[官方价格](https://developers.cloudflare.com/r2/pricing/)

本方案迁移的是固定资源。上游主包、安装器更新和签名清单仍按原渠道工作。上游主包及 REFramework 仍以 ZIP 下载，并调用 `src/archive.rs` 的 `extract_zip`，因此 `Cargo.toml` 中的 Rust `zip` 依赖仍需保留；这里指多个安装流程复用的解压代码，不是系统里的共享 DLL。CDN 缓存与 Windows 构建验收均已通过。

## 9. 当前实施记录

- 普通回归测试 46 项通过；新增两个测试覆盖清单/旧设置、逐文件续传/缓存复用/损坏拦截。手动联网安装测试默认忽略，需要显式启用。
- 格式检查与 Clippy `-D warnings` 通过；发布脚本的损坏源文件拒绝检查通过。
- 三个安装资源和两份源材料内容均保持原哈希，已分别上传；许可说明也已验证。
- Windows CI [39358001313](https://github.com/DestinySail/optiscaler-nr-installer/actions/runs/39358001313) 全部通过；0.2.2 使用该 CI 产物，更新清单 minisign 验签及草稿附件回读校验通过。EXE SHA-256 为 `ad639cbbc8bfdc8d5cbea7cf1ace5ef21ac5b2e31ccfc1508b2dd15db8493d14`。
- 显式联网验收已通过：从 CDN 取得真实 1 MiB 前缀后续传插件，逐个下载并验证三个资源，在隔离临时目录安装、检查和卸载；准备阶段约 69.8 秒，全程约 90 秒。旧设置中的不可达 ZIP URL 没有影响下载；两个源材料没有部署到测试游戏目录；卸载后只保留原始惰性 PE 测试文件。未启动或修改真实游戏。
- 用户已在控制台启用 `OptiScaler downloads` 缓存规则：`http.host eq "downloads.bonvoyage.dpdns.org" and starts_with(http.request.uri.path, "/optiscaler-nr/")`，动作设为 Eligible for cache，Edge TTL 尊重源站。六个文件均已实测命中缓存，包含 `.asi`、`.dll` 和许可文本。
- [0.2.2](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/app-v0.2.2) 已正式发布；更新目录工作流成功，公开清单版本为 0.2.2 且签名有效。正式发布 EXE 在隔离目录完成 Cloudflare 下载、841 个受管文件安装、检查和卸载；旧 ZIP 配置未参与下载，源材料未安装，惰性测试文件未变。
- Wrangler OAuth 缺少 Rulesets 权限，Computer Use 读取 Chrome URL 失败，因此缓存规则由用户在已登录的外部浏览器完成；未新增 API Token 或安装浏览器扩展。

手动联网验收命令（使用已校验的 v1.12.2 主包 ZIP，下载约 200 MiB 固定资源，仅安装到临时测试目录，不运行游戏）：

```powershell
$env:NR_TEST_UPSTREAM_ZIP = 'C:\path\to\OptScaler-NR-1.12.2.zip'
cargo test --locked assets::tests::cloudflare_live_install_and_uninstall -- --ignored --nocapture
```
