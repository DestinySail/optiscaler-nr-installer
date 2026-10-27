# OptiScaler NR Installer 0.2.7

- 所有网络请求改为跟随 Windows 系统代理（注册表 `ProxyEnable`/`ProxyServer`）。此前 `reqwest` 构建时关掉了系统代理特性，安装器只用直连，开启代理的用户下载主包、固定资源、REFramework 和安装器更新时无法借力代理。
- 同一环境实测：直连约 0.05 MiB/s，走本机代理约 2.6 MiB/s，205 MiB 主包从一小时以上降到约 80 秒。
- 环境变量 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` 的优先级高于系统配置，仍然生效。
- 断点续传、大小与 SHA-256 校验逻辑不变；已存在的 `.part` 文件继续复用。

已知上限：`ProxyServer` 若写成分协议格式 `http=host:port;https=host:port`，`reqwest` 无法解析会退回直连。

验证：52 项自动测试、`cargo clippy --all-targets -- -D warnings`、Release 构建，以及本机代理与直连的 A/B 下载实测。
