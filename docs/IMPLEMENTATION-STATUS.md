# 实施状态

日期：2026-10-17

## 已完成

- Rust / eframe 桌面程序，正常流程为“选择游戏 → 安装进度 → 完成”。
- DXGI 枚举 GPU；本机识别到 RX 9070 XT（1002:7550）和软件适配器，RDNA4 条件通过。
- Steam、Epic、Windows 卸载记录和手动 EXE 发现。本机已验证从 WeGame 卸载记录定位鸣潮真实 EXE。
- 唯一外部组件上游为 `TheAutomatic/dlss-5-amd-project`，匹配稳定版 `OptScaler-NR-*.zip` 和 GitHub SHA-256。
- 固定 `extras-v1` Release 资源设计；客户端不查询插件、模型或其他组件仓库。
- 下载断点、重试、大小与哈希校验；安全 ZIP 解压；安装前备份；事务日志；失败恢复。
- Windows 命名互斥量阻止两个安装器实例并发修改游戏或安装记录。
- Mochizuki + XeFG 固定 INI 合并；检查、修复、更新、回滚、关闭插帧和卸载。
- 自有 `app-v*` Release 的签名清单检查、下载、哈希校验和 `self-replace` 更新入口。
- CI 构建工作流、资源打包脚本和应用发布清单脚本。

## 已验证

- `cargo test`：9 项通过，包含 minisign 清单验签、篡改拒绝、单实例锁和 OptiScaler 标记识别。
- `cargo clippy --all-targets -- -D warnings`：通过。
- upstream v1.12.2：实际下载 203.7 MiB 压缩包，解压 990 项成功。
- 在独立测试目录安装 840 个受管文件，检查通过。
- v1.12.2 → v1.12.1 → 回滚 v1.12.2：检查通过。
- 关闭插帧：MFGUnlocker.asi 移出，`[FrameGen] Enabled=false`。
- 卸载：840 个受管文件按基线清除，原始 `DummyGame.exe` 保留。
- 代理冲突：预先放入第三方 `dxgi.dll` 时选择 `winmm.dll`，第三方文件哈希不变，自定义 INI 段保留。
- 旧安装接管：预先放入已有 OptiScaler `dxgi.dll` 时识别并继续使用 `dxgi.dll`，基线备份哈希一致，卸载后恢复原文件。
- 检查/修复：篡改 `dlssnr-amd/shaders/accumulation.txt` 后检查报告 1 个问题，修复后检查通过。
- 中途失败：锁定 `MFGUnlocker.asi` 令替换失败，已写入的代理、INI、runtime 被回滚，原锁定文件保留，事务日志清除。
- `extras-v1.zip`：317,857,005 字节，SHA-256 `f3d649477ee83a8281e3f5e7cbc45c25486289c3a0b6b0fa860fa725ad8f99bf`。
- Release EXE：`OptiScalerNR.exe`，SHA-256 `40056129f1700ea1c843812fda080d32a11982a065b476c7a34e230806f7e0a1`。

测试没有修改真实鸣潮目录，也没有启动游戏。

## 发布前剩余

1. 确认 GitHub 仓库地址。当前代码默认使用 `DestinySail/optiscaler-nr-installer`。
2. 创建 `extras-v1` Release，上传 `dist/extras-v1.zip`、`dist/SHA256SUMS.txt` 和 `dist/extras-v1.sha256`。
3. 创建 `app-v0.1.0` Release，上传 `dist/OptiScalerNR.exe`、`dist/update-manifest.json` 和 minisign 签名。
4. 生成 minisign 密钥，将私钥只保存在发布环境，把公钥通过 `NR_UPDATE_PUBKEY` 编译进正式 EXE。
5. 至少再选两款游戏完成真实游戏内 NR + XeFG 验证；未实测的游戏继续显示“候选”。
