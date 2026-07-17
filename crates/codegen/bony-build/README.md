# bony-build

**Bony Build** 桌面客户端 crate（eframe/egui + ACP stdio）。

产品说明、截图与完整上手步骤见仓库根目录 [`README.md`](../../../README.md)。

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
