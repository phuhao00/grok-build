# bony-build

**Bony Build** 桌面客户端 crate（eframe/egui + ACP stdio）。

产品说明、截图与完整上手步骤见仓库根目录 [`README.md`](../../../README.md)（含 Unity、任务 / worktree、监控等介绍）。

## 运行

```powershell
# 仓库根目录
powershell -ExecutionPolicy Bypass -File .\scripts\run-desktop.ps1
# 或
cargo run -p bony-build
```

```text
--cwd <path>           会话工作目录
--grok-bin <path>      grok 可执行文件
--ask-permissions      工具需手动批准
```

## 结构

```text
Bony Build (egui)
    │  ACP JSON-RPC over stdio
    ▼
grok agent stdio  →  MvpAgent / SessionActor
```

本 crate 不嵌入完整 agent 运行时，只作为桌面壳驱动 `grok` 子进程。

## Unity 控制

入口有两处：

1. **侧栏「Unity 控制」** — 引导安装 CLI / Pipeline、选工程、按钮操作  
2. **聊天输入框旁的 `Unity` 按钮** — 打开对话控制芯片；也可直接发送「探测编辑器」「进入 Play」或 `/unity`

对话控制走本地 Unity CLI，**不经 Agent**，避免 agent 在 worktree 里挂死 `unity pipeline install`。

引导步骤：

1. 安装 Unity CLI（复制安装命令）
2. 重新检测
3. 确认 Unity 项目目录
4. 安装 Pipeline（`unity pipeline install`）
5. 探测编辑器（需编辑器已打开项目）
6. 跑完整闭环

Windows 默认安装路径：`%LOCALAPPDATA%\Unity\bin\unity.exe`。

安装 CLI：

```powershell
$env:UNITY_CLI_CHANNEL='beta'; irm https://public-cdn.cloud.unity3d.com/hub/prod/cli/install.ps1 | iex
```
