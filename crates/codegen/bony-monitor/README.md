# bony-monitor

Bony Build 的本地 Web 监控：架构总览 + Git 改动影响时间线 + 功能矩阵。

## 自动更新

| 时机 | 行为 |
|------|------|
| 启动前 | `run-monitor.ps1` 调用 `sync-monitor-catalog.ps1` 扫描模块 → `catalog/discovered.json` |
| 运行中 | 每次 API 检查 `features.toml` / 源码目录 mtime，热重载规则并重扫模块；`git log` 每次现拉 |
| 前端 | 约 12 秒轮询，看板开着改代码也能看到新 commit / `auto-*` 模块 |

人工语义写在 [`catalog/features.toml`](catalog/features.toml)。未覆盖的路径会合成 `auto-<crate>-<stem>`，建议随后并入 TOML。

```powershell
# 仓库根目录
powershell -ExecutionPolicy Bypass -File .\scripts\run-monitor.ps1
# 打开 http://127.0.0.1:8787
```

仅同步目录：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\sync-monitor-catalog.ps1
```

可选在 commit message 中标注：

```text
Impact: 改善桌面模型切换体验
风险: 需回归登录流程
```
