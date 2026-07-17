//! Illustrated workflow page: question + storyboard (image + steps).

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkflowStep {
    pub n: u32,
    pub title: String,
    pub actor: String,
    pub action: String,
    pub artifact: String,
    pub crates: Vec<String>,
    /// Short label for the visual pipeline chip.
    pub chip: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowScene {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// Optional illustration path (served under /repo-docs).
    pub image: Option<String>,
    pub image_caption: Option<String>,
    /// left | right — image placement relative to text.
    pub image_side: String,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowGalleryItem {
    pub title: String,
    pub path: String,
    pub caption: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowOverview {
    pub question: String,
    pub answer: String,
    pub blurb: String,
    /// Top visual pipeline (derived from first scene steps in UI if empty).
    pub pipeline: Vec<String>,
    pub scenes: Vec<WorkflowScene>,
    pub gallery: Vec<WorkflowGalleryItem>,
}

pub fn overview() -> WorkflowOverview {
    WorkflowOverview {
        question: "项目是怎么工作的？".into(),
        answer: "用户在桌面壳里下一句任务，经 ACP 交给 grok agent；SessionActor 循环「采样 → 工具 → 再采样」，结束后用量落盘，监控看板解释改动影响。".into(),
        blurb: "图文分镜：左/右配图 + 逐步调用链，可对照真实界面与架构图阅读。".into(),
        pipeline: vec![
            "输入任务".into(),
            "ACP 桥接".into(),
            "Session".into(),
            "采样模型".into(),
            "执行工具".into(),
            "落盘记账".into(),
        ],
        scenes: vec![
            WorkflowScene {
                id: "ask".into(),
                title: "分镜一 · 一次提问从进到出".into(),
                summary: "以 Bony Build 桌面端 + config.toml 默认模型（如 qwen-max）为例。".into(),
                image: Some("/repo-docs/bony-build-desktop.png".into()),
                image_caption: Some("桌面端：对话、侧栏任务、模型选择与用量入口".into()),
                image_side: "right".into(),
                steps: vec![
                    step(
                        1,
                        "输入任务",
                        "用户输入任务",
                        "Bony Build UI",
                        "在悬浮输入框按 Enter；可用 config.toml 的 env_key（如 DASHSCOPE_API_KEY）完成 BYOK。",
                        "UiCommand::Prompt(text)",
                        &["bony-build"],
                    ),
                    step(
                        2,
                        "ACP",
                        "桌面桥接发出 ACP prompt",
                        "agent_bridge",
                        "子进程已跑 grok agent stdio；bridge 把文本打成 ACP PromptRequest。",
                        "ACP JSON-RPC → grok stdin",
                        &["bony-build", "xai-acp-lib"],
                    ),
                    step(
                        3,
                        "Session",
                        "Session 接住 turn",
                        "SessionActor / MvpAgent",
                        "创建或复用 session，装好系统提示、工具集与当前模型；开始 agentic loop。",
                        "session_id + ChatState",
                        &["xai-grok-shell", "xai-chat-state"],
                    ),
                    step(
                        4,
                        "采样",
                        "采样模型",
                        "Sampler",
                        "按模型 backend 发 HTTP/SSE；流式文本块经 ACP 推回桌面。",
                        "AssistantDelta + tool_calls",
                        &["xai-grok-sampler", "xai-grok-sampling-types"],
                    ),
                    step(
                        5,
                        "工具",
                        "执行工具（如有）",
                        "ToolBridge",
                        "改文件/跑命令在 Workspace 权限与沙箱内落地；结果回灌后再采样。",
                        "tool result → next messages",
                        &["xai-grok-tools", "xai-tool-runtime", "xai-grok-workspace"],
                    ),
                    step(
                        6,
                        "落盘",
                        "轮次结束并记账",
                        "desktop + storage",
                        "stop_reason / token usage 回 UI；追加 turns.jsonl，用量面板画趋势。",
                        "~/.bony-build/turns.jsonl",
                        &["bony-build"],
                    ),
                ],
            },
            WorkflowScene {
                id: "turn".into(),
                title: "分镜二 · Agent turn 怎么转".into(),
                summary: "采样与工具在 Session 内交替，直到模型不再发起 tool call。".into(),
                image: Some("/repo-docs/architecture-turn-flow.png".into()),
                image_caption: Some("Turn 流程：采样 → 工具 → 再采样".into()),
                image_side: "left".into(),
                steps: vec![
                    step(
                        1,
                        "装配",
                        "Agent 已装配好",
                        "xai-grok-agent",
                        "AgentBuilder 造出带提示词与工具的 Agent；不负责跑 loop。",
                        "Agent + ToolBridge",
                        &["xai-grok-agent"],
                    ),
                    step(
                        2,
                        "循环",
                        "SessionActor 跑 loop",
                        "xai-grok-shell",
                        "真正的 agentic turn：采样 → 工具 → 再采样，旁路含 memory / compaction。",
                        "turn events via ACP",
                        &["xai-grok-shell"],
                    ),
                ],
            },
            WorkflowScene {
                id: "layers".into(),
                title: "分镜三 · 分层放在哪".into(),
                summary: "从上到下：Host 客户端 → Session → Agent / 采样 / 工具 → Workspace。".into(),
                image: Some("/repo-docs/architecture-layers.png".into()),
                image_caption: Some("分层架构总览（与 ARCHITECTURE.md 同源）".into()),
                image_side: "right".into(),
                steps: vec![
                    step(
                        1,
                        "Host",
                        "Host / 客户端",
                        "bony-build / TUI",
                        "只负责界面与 ACP；不内嵌完整 agent 运行时。",
                        "desktop or pager UI",
                        &["bony-build", "xai-grok-pager"],
                    ),
                    step(
                        2,
                        "Runtime",
                        "Session + 三件套",
                        "shell / sampler / tools",
                        "Session 托管 loop；采样打模型；工具改工作区。",
                        "stdio agent process",
                        &["xai-grok-shell", "xai-grok-sampler", "xai-grok-tools"],
                    ),
                    step(
                        3,
                        "WS",
                        "Workspace 落地",
                        "workspace / sandbox",
                        "权限、沙箱、文件系统、checkpoint 约束工具副作用。",
                        "FS + VCS + permissions",
                        &["xai-grok-workspace", "xai-grok-sandbox"],
                    ),
                ],
            },
            WorkflowScene {
                id: "switch".into(),
                title: "分镜四 · 换项目 / 换模型".into(),
                summary: "cwd 与默认模型变更会重建 bridge，而不是热改全局单例。".into(),
                image: Some("/repo-docs/bony-build-desktop.png".into()),
                image_caption: Some("侧栏项目与底部模型选择器".into()),
                image_side: "left".into(),
                steps: vec![
                    step(
                        1,
                        "换目录",
                        "切换工作目录",
                        "Bony Build",
                        "Shutdown 旧 agent → 新 cwd 再 spawn grok agent stdio → session/new。",
                        "new session_id",
                        &["bony-build"],
                    ),
                    step(
                        2,
                        "换模型",
                        "切换默认模型",
                        "model picker",
                        "ACP session/set_model，并写回 ~/.grok/config.toml 的 [models] default。",
                        "config.toml default",
                        &["bony-build", "xai-grok-shell"],
                    ),
                ],
            },
            WorkflowScene {
                id: "monitor".into(),
                title: "分镜五 · 监控看板接到哪里".into(),
                summary: "bony-monitor 不跑 agent；读 git + features.toml，解释改动影响。".into(),
                image: None,
                image_caption: None,
                image_side: "right".into(),
                steps: vec![
                    step(
                        1,
                        "扫描",
                        "读 git + 功能目录",
                        "bony-monitor",
                        "扫描近期 commit，用 features.toml 匹配路径与关键词。",
                        "/api/changes · /api/features",
                        &["bony-monitor"],
                    ),
                    step(
                        2,
                        "影响",
                        "生成影响摘要",
                        "impact engine",
                        "标出产品功能、风险维度与建议回归清单。",
                        "timeline + drawer",
                        &["bony-monitor"],
                    ),
                ],
            },
        ],
        gallery: vec![
            WorkflowGalleryItem {
                title: "桌面端界面".into(),
                path: "/repo-docs/bony-build-desktop.png".into(),
                caption: "用户从这里发起提问与切换模型".into(),
            },
            WorkflowGalleryItem {
                title: "Turn 流程".into(),
                path: "/repo-docs/architecture-turn-flow.png".into(),
                caption: "一次 turn 内采样与工具如何交替".into(),
            },
            WorkflowGalleryItem {
                title: "分层架构".into(),
                path: "/repo-docs/architecture-layers.png".into(),
                caption: "Host → Session → Agent/采样/工具 → Workspace".into(),
            },
        ],
    }
}

fn step(
    n: u32,
    chip: &str,
    title: &str,
    actor: &str,
    action: &str,
    artifact: &str,
    crates: &[&str],
) -> WorkflowStep {
    WorkflowStep {
        n,
        chip: chip.into(),
        title: title.into(),
        actor: actor.into(),
        action: action.into(),
        artifact: artifact.into(),
        crates: crates.iter().map(|s| (*s).into()).collect(),
    }
}
