# OptiScaler NR Installer 0.2.0

- 维护页新增“安装 REFramework”。
- 从 `praydog/REFramework-nightly` 最新 Release 下载并校验 `REFramework.zip`。
- 只部署根目录的 `dinput8.dll` 到游戏 EXE 旁，用于修复 PRAGMATA 等 RE Engine 游戏加载模组后启动自动关闭。
- 安装状态使用独立的 `reframework.json`；游戏卸载时只移除未被修改的受管 `dinput8.dll`。
- 安装器更新进度条不再使用旋转动画，下方加载状态保持不变。

REFramework nightly 是滚动发布，不是稳定的游戏兼容性承诺。主包安装与更新行为保持不变。
