# `extras-v1` 来源与发布边界

该资源包不是安装器 MIT 许可证的一部分。各文件继续适用其原项目许可证；公开发布前需要由维护者确认再分发条件。

| 文件 | 来源/用途 | 安装器行为 |
|---|---|---|
| `MFGUnlocker.asi` | dashdogy/MFG-Unlocker，当前包标记 v1.5.0 | 部署到 `OptiScaler/plugins/` |
| `dlssnr-amd/dlssnr.bin` | 由 Mochizuki 工具从 NVIDIA 310.8.0 DLSS-NR 运行库提取，主包附带的模型 | 部署到 `dlssnr-amd/` |
| `nvngx_dlssnr.dll` | NVIDIA DLSS-NR 310.8.0，作为固定源材料存档 | 当前固定方案不部署 |
| `dlssnr_on_amd_weights.bin` | danielblnc/DLSS-NR-on-AMD 后端权重，当前方案不使用 | 当前固定方案不部署 |

相关项目：

- <https://github.com/dashdogy/MFG-Unlocker>
- <https://github.com/danielblnc/DLSS-NR-on-AMD>
- <https://github.com/TheAutomatic/dlss-5-amd-project>

NVIDIA 运行库和模型权重可能受额外许可约束；没有明确再分发授权时，应改为让用户自行提供源文件，或从有权分发的来源取得。
