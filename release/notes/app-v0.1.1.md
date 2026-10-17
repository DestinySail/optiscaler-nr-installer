# OptiScaler NR Installer 0.1.1

修复启动或执行维护操作时短暂出现控制台窗口的问题。

游戏发现调用的 `reg.exe` 和进程检查调用的 `tasklist.exe` 现在使用
`CREATE_NO_WINDOW`，不会再为 GUI 安装器创建可见控制台。

安装方案、资源版本和兼容范围与 0.1.0 相同。
