# 实施状态

日期：2026-10-17

## 已完成

- Rust / eframe 桌面程序，正常流程为“选择游戏 → 安装进度 → 完成”。
- 0.1.3 取消游戏文件备份和上一版本快照：更新先清理旧注入，卸载删除本项目记录的文件；升级时自动删除旧版遗留备份。
- 0.1.2 修复 Steam 库漏扫，支持从多个 Steam 库清单发现小型 64 位游戏，并为手动选择 EXE 设置确定的起始目录。
- 0.1.1 修复 GUI 版启动和扫描时短暂闪现 `reg.exe` / `tasklist.exe` 控制台窗口的问题。
- DXGI 枚举 GPU；本机识别到 RX 9070 XT（1002:7550）和软件适配器，RDNA4 条件通过。
- Steam、Epic、Windows 卸载记录和手动 EXE 发现。本机已验证从 WeGame 卸载记录定位鸣潮真实 EXE。
- 唯一外部组件上游为 `TheAutomatic/dlss-5-amd-project`，匹配稳定版 `OptScaler-NR-*.zip` 和 GitHub SHA-256。
- 固定 `extras-v1` Release 资源设计；客户端不查询插件、模型或其他组件仓库。
- 下载断点、重试、大小与哈希校验；安全 ZIP 解压；安装前清理旧注入；事务日志；失败后移除注入。
- Windows 命名互斥量阻止两个安装器实例并发修改游戏或安装记录。
- Mochizuki + XeFG 固定 INI 合并；检查、修复、更新、关闭插帧和卸载。
- 自有 `app-v*` Release 的签名清单检查、下载、哈希校验和 `self-replace` 更新入口。
- CI 构建工作流、资源打包脚本和应用发布清单脚本。
- 公开仓库和四个应用 Release 已发布：
  - [仓库](https://github.com/DestinySail/optiscaler-nr-installer)
  - [app-v0.1.0](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/app-v0.1.0)
  - [app-v0.1.1](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/app-v0.1.1)
  - [app-v0.1.2](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/app-v0.1.2)
  - [app-v0.1.3](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/app-v0.1.3)
  - [extras-v1](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/extras-v1)
  - [extras-maintenance-v1](https://github.com/DestinySail/optiscaler-nr-installer/releases/tag/extras-maintenance-v1)

## 已验证

- `cargo test`：16 项通过，包含 Steam 清单解析、辅助 EXE 过滤、无备份卸载、失败事务清理、旧备份清理、minisign 清单验签、篡改拒绝、单实例锁、OptiScaler 标记识别和用户偏好保留。
- `cargo clippy --all-targets -- -D warnings`：通过。
- upstream v1.12.2：实际下载 203.7 MiB 压缩包，解压 990 项成功。
- 在独立测试目录安装 840 个受管文件，检查通过。
- 新版 `extras-v1.zip` 安装 841 个文件，`dlssnr-amd/dlssnr-int4.bin` 哈希与上游清单一致。
- 安装前还会读取主包 `model-tools/dlssnr-int4.sha256`，固定 INT4 模型不匹配时停止，避免静默混用版本。
- 在独立测试目录连续安装两次 v1.12.2：旧注入目录被清理，陈旧文件消失，新增备份文件数为 0。
- 关闭插帧：MFGUnlocker.asi 移出，`[FrameGen] Enabled=false`。
- 卸载：841 个注入文件删除，`DummyGame.exe` 保留，游戏目录恢复为安装前结构。
- 代理冲突：预先放入第三方 `dxgi.dll` 时选择 `winmm.dll`，第三方文件哈希不变，自定义 INI 段保留。
- 旧安装接管：预先放入已有 OptiScaler `dxgi.dll` 时识别并继续使用 `dxgi.dll`；卸载时删除该注入文件，不再恢复旧版本。
- 检查/修复：篡改 `dlssnr-amd/shaders/accumulation.txt` 后检查报告 1 个问题，修复后检查通过。
- INI：游戏回写无关键（`HighResMV=true`）后检查仍通过；关闭插帧后检查也按禁用状态通过。
- 事务清理：无备份事务记录会删除本次写入的注入文件，不读取旧版本快照。
- 发布清单：使用正式 minisign 发布密钥签名 `update-manifest.json`，本地和远端下载后均验签通过。
- 远端资源：安装器从公开 `extras-v1` Release 下载 145.4 MiB 资源，完成 841 个文件安装、检查和卸载。
- 已发布的 0.1.0 安装器能从 GitHub 识别并验签 0.1.1 更新。
- GitHub Actions 的 `main` 构建、测试和 Clippy 检查通过。
- 必装 `extras-v1.zip`：152,492,907 字节，SHA-256 `ec993a9a75772c2a178d167be43a1760218001a68e6a367f1caa93be9e400533`，包含已校验的 `dlssnr-int4.bin` 和来源说明。
- 维护 `extras-maintenance-v1.zip`：212,830,627 字节，SHA-256 `0e36bfaf53ec4e4c74d463583714be8570a3abe49475c4b28f12b1104f169de2`，安装器不下载。
- Release EXE 0.1.1：SHA-256 `710700ab563770603781f50d4e991c9dd0903c82deaf3f6920ee0c985429edd5`，内置更新公钥 `A97B5A26BCAA69D9`。
- Release EXE 0.1.2：SHA-256 `7a64fa5f5f4d7655697cb7b6574d49a8edcd71079da051190d166fe585c52586`；远端重新下载的 EXE 哈希一致，签名更新清单使用正式公钥验签通过。
- Release EXE 0.1.3：SHA-256 `41ac7d34c8e3e466957a0c677433a9068d808d64a99760448058ad61f5e21236`，签名更新清单使用正式公钥验签通过。
- 旧版 `backups` 目录中的 6,728 个文件已删除，`backups` 目录不再存在；新安装和更新流程不会重新创建它。
- 本机 Steam 清单中的 13 款 64 位游戏全部被识别，32 位《雀魂》因固定方案不支持而被排除；WeGame 版鸣潮继续从卸载记录定位。

测试没有修改真实鸣潮目录，也没有启动游戏。

## 待验证

1. 在真实鸣潮中完成 NR 与 XeFG 的启动、画面和稳定性验收。
2. 再选两款游戏完成真实游戏内验证；在完成前，它们继续显示为“候选”。
3. 在 GUI 中演练一次完整的 0.1.2 → 0.1.3 下载、替换和重启流程。
