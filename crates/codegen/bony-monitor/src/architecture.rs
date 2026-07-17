//! Architecture overview for the dashboard (refreshed with discovered modules).

use serde::Serialize;

use crate::catalog::CatalogSnapshot;

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

pub fn overview(catalog: &CatalogSnapshot) -> ArchitectureOverview {
    let mut host_crates = vec![
        "bony-build".into(),
        "bony-monitor".into(),
        "xai-grok-pager*".into(),
    ];
    if catalog.desktop_module_count > 0 {
        host_crates.push(format!("bony-build src ×{}", catalog.desktop_module_count));
    }
    for d in catalog
        .discovered
        .iter()
        .filter(|d| d.crate_name == "bony-build")
        .take(8)
    {
        host_crates.push(format!("· {}", d.stem));
    }

    ArchitectureOverview {
        title: "Bony Build 架构".into(),
        blurb: "Codex 式桌面壳经 ACP 驱动 grok agent stdio；目录与模块扫描会随工作区热更新。".into(),
        layers: vec![
            ArchLayer {
                id: "host".into(),
                name: "1 · Host / 客户端".into(),
                summary: format!(
                    "Bony Build 桌面（顶栏/侧栏/项目/用量图）、TUI、或任意 ACP 客户端；当前扫描到 {} 个桌面模块",
                    catalog.desktop_module_count
                ),
                crates: host_crates,
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
            "轮次结束写入 ~/.bony-build/turns.jsonl；使用统计面板展示折线/柱状趋势".into(),
            "切换项目 → Shutdown 旧 bridge → 新 cwd 重连 agent".into(),
            "bony-monitor 热重载 features.toml / 扫描模块，并按规则生成影响摘要".into(),
        ],
    }
}
