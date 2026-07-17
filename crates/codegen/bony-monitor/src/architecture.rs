//! Static architecture overview for the dashboard.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ArchLayer {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub crates: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ArchDiagram {
    pub id: String,
    pub title: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ArchitectureOverview {
    pub title: String,
    pub blurb: String,
    pub layers: Vec<ArchLayer>,
    pub diagrams: Vec<ArchDiagram>,
    pub flows: Vec<String>,
}

pub fn overview() -> ArchitectureOverview {
    ArchitectureOverview {
        title: "Bony Build 架构".into(),
        blurb: "桌面壳经 ACP 驱动 grok agent stdio；SessionActor 负责采样→工具→再采样循环。".into(),
        layers: vec![
            ArchLayer {
                id: "host".into(),
                name: "1 · Host / 客户端".into(),
                summary: "用户入口：Bony Build 桌面、TUI、或任意 ACP 客户端".into(),
                crates: vec![
                    "bony-build".into(),
                    "bony-monitor".into(),
                    "xai-grok-pager*".into(),
                ],
            },
            ArchLayer {
                id: "session".into(),
                name: "2 · Session".into(),
                summary: "SessionActor 跑 agentic turn：采样 → 工具 → 再采样".into(),
                crates: vec!["xai-grok-shell".into(), "xai-chat-state".into()],
            },
            ArchLayer {
                id: "agent".into(),
                name: "3a · Agent 定义".into(),
                summary: "AgentBuilder 装配提示词与工具集（不负责 loop）".into(),
                crates: vec!["xai-grok-agent".into()],
            },
            ArchLayer {
                id: "sampling".into(),
                name: "3b · 采样".into(),
                summary: "多 backend HTTP/SSE：OpenAI 兼容 / Anthropic Messages 等".into(),
                crates: vec!["xai-grok-sampler".into(), "xai-grok-sampling-types".into()],
            },
            ArchLayer {
                id: "tools".into(),
                name: "3c · 工具".into(),
                summary: "终端、文件编辑、搜索等 tool call 落地".into(),
                crates: vec![
                    "xai-grok-tools".into(),
                    "xai-tool-runtime".into(),
                    "xai-tool-protocol".into(),
                ],
            },
            ArchLayer {
                id: "workspace".into(),
                name: "4 · Workspace".into(),
                summary: "权限、沙箱、文件系统、checkpoint".into(),
                crates: vec!["xai-grok-workspace".into(), "xai-grok-sandbox".into()],
            },
        ],
        diagrams: vec![
            ArchDiagram {
                id: "layers".into(),
                title: "分层架构".into(),
                path: "/repo-docs/architecture-layers.png".into(),
            },
            ArchDiagram {
                id: "turn".into(),
                title: "Turn 流程".into(),
                path: "/repo-docs/architecture-turn-flow.png".into(),
            },
            ArchDiagram {
                id: "desktop".into(),
                title: "桌面端界面".into(),
                path: "/repo-docs/bony-build-desktop.png".into(),
            },
        ],
        flows: vec![
            "用户在 Bony Build 输入任务 → ACP prompt → SessionActor".into(),
            "Sampler 请求当前模型（如 qwen-max）→ 流式文本 / tool calls".into(),
            "ToolBridge 执行工具 → 结果回灌 → 继续采样直到结束".into(),
            "bony-monitor 读取 git 历史，按路径规则生成影响摘要".into(),
        ],
    }
}
