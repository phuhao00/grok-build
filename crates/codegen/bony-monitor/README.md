# bony-monitor

Bony Build 的本地 Web 监控：架构总览 + **怎么工作**（一次提问的端到端流程）+ Git 改动影响时间线 + 功能矩阵。

顶部页签：

| 页 | 内容 |
|---|---|
| 总览 | 功能影响矩阵 + 分层架构 |
| 怎么工作 | 时序图 / 分层图 / 调用图 / 模块条形图 + 源码地图 + 分镜调用链 |
| 改动 | 时间线与影响抽屉 |

API：`GET /api/workflow`（含 `charts`、`code_map`，模块数随扫描热更新）

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
