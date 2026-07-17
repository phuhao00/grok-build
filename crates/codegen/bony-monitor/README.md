# bony-monitor

Bony Build 的本地 Web 监控：架构总览 + Git 改动影响时间线。

```powershell
# 仓库根目录
powershell -ExecutionPolicy Bypass -File .\scripts\run-monitor.ps1
# 打开 http://127.0.0.1:8787
```

可选在 commit message 中标注：

```text
Impact: 改善桌面模型切换体验
风险: 需回归登录流程
```
