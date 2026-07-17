//! How-it-works page: storyboard + chart-backed code map (live from catalog).

use serde::Serialize;

use crate::catalog::{CatalogSnapshot, DiscoveredFile};

#[derive(Debug, Serialize)]
pub struct WorkflowStep {
    pub n: u32,
    pub title: String,
    pub actor: String,
    pub action: String,
    pub artifact: String,
    pub crates: Vec<String>,
    pub chip: String,
    /// Repo-relative source anchors for this step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code_refs: Vec<CodeRef>,
}

#[derive(Debug, Serialize, Clone)]
pub struct CodeRef {
    pub path: String,
    pub symbol: String,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowScene {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub image: Option<String>,
    pub image_caption: Option<String>,
    pub image_side: String,
    pub steps: Vec<WorkflowStep>,
    /// Chart ids rendered inside this scene body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chart_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowGalleryItem {
    pub title: String,
    pub path: String,
    pub caption: String,
}

#[derive(Debug, Serialize)]
pub struct SeqMessage {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct LayerBand {
    pub name: String,
    pub summary: String,
    pub items: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BarItem {
    pub label: String,
    pub value: u64,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowChart {
    pub id: String,
    pub title: String,
    pub caption: String,
    /// sequence | layers | flow | bars | callgraph
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<SeqMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bands: Vec<LayerBand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<FlowNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<FlowEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bars: Vec<BarItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeModule {
    pub path: String,
    pub stem: String,
    pub crate_name: String,
    pub layer: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_types: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowOverview {
    pub question: String,
    pub answer: String,
    pub blurb: String,
    pub pipeline: Vec<String>,
    pub scenes: Vec<WorkflowScene>,
    pub gallery: Vec<WorkflowGalleryItem>,
    pub charts: Vec<WorkflowChart>,
    pub code_map: Vec<CodeModule>,
    pub module_count: usize,
    pub desktop_module_count: usize,
}

pub fn overview(catalog: &CatalogSnapshot) -> WorkflowOverview {
    let code_map = build_code_map(catalog);
    let charts = build_charts(catalog, &code_map);

    WorkflowOverview {
        question: "项目是怎么工作的？".into(),
        answer: "用户在桌面壳里下一句任务，经 ACP 交给 grok agent；SessionActor 循环「采样 → 工具 → 再采样」，结束后用量落盘，监控看板解释改动影响。".into(),
        blurb: "下面用时序图、分层图、调用图和源码地图对照真实 crate / 文件；模块列表随工作区扫描热更新。".into(),
        pipeline: vec![
            "输入任务".into(),
            "ACP 桥接".into(),
            "Session".into(),
            "采样模型".into(),
            "执行工具".into(),
            "落盘记账".into(),
        ],
        scenes: scenes(),
        gallery: gallery(),
        charts,
        module_count: catalog.discovered.len(),
        desktop_module_count: catalog.desktop_module_count,
        code_map,
    }
}

fn scenes() -> Vec<WorkflowScene> {
    vec![
        WorkflowScene {
            id: "ask".into(),
            title: "分镜一 · 一次提问从进到出".into(),
            summary: "桌面 UI → ACP → Session → Sampler → Tools → turns.jsonl，对照时序图与源码锚点。".into(),
            image: Some("/repo-docs/bony-build-desktop.png".into()),
            image_caption: Some("桌面端：对话、侧栏任务、模型选择与用量入口".into()),
            image_side: "right".into(),
            chart_ids: vec!["seq-prompt".into(), "flow-prompt".into()],
            steps: vec![
                step(
                    1,
                    "输入任务",
                    "用户输入任务",
                    "Bony Build UI",
                    "悬浮输入框 Enter；BYOK 可用 config.toml 的 env_key（如 DASHSCOPE_API_KEY）。",
                    "UiCommand::Prompt(text)",
                    &["bony-build"],
                    &[
                        cref(
                            "crates/codegen/bony-build/src/app.rs",
                            "UiCommand::Prompt",
                            "收集输入并发给 bridge",
                        ),
                        cref(
                            "crates/codegen/bony-build/src/events.rs",
                            "UiCommand / AgentEvent",
                            "UI ↔ bridge 通道契约",
                        ),
                    ],
                ),
                step(
                    2,
                    "ACP",
                    "桌面桥接发出 ACP prompt",
                    "agent_bridge",
                    "子进程已跑 grok agent stdio；bridge 把文本打成 ACP PromptRequest。",
                    "ACP JSON-RPC → grok stdin",
                    &["bony-build", "xai-acp-lib"],
                    &[
                        cref(
                            "crates/codegen/bony-build/src/agent_bridge.rs",
                            "spawn_bridge / prompt",
                            "stdio 子进程与 session/prompt",
                        ),
                        cref(
                            "crates/codegen/bony-build/src/config_io.rs",
                            "hydrate_model_env_keys",
                            "把 User 环境密钥灌进进程",
                        ),
                    ],
                ),
                step(
                    3,
                    "Session",
                    "Session 接住 turn",
                    "SessionActor / MvpAgent",
                    "创建或复用 session，装好系统提示、工具集与当前模型；开始 agentic loop。",
                    "session_id + ChatState",
                    &["xai-grok-shell", "xai-chat-state"],
                    &[
                        cref(
                            "crates/codegen/xai-grok-shell/src/session/acp_session.rs",
                            "SessionActor",
                            "持有 agent / chat_state / sampler",
                        ),
                        cref(
                            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/turn.rs",
                            "handle_prompt",
                            "单条用户消息 turn 入口",
                        ),
                    ],
                ),
                step(
                    4,
                    "采样",
                    "采样模型",
                    "Sampler",
                    "按模型 backend 发 HTTP/SSE；流式文本块经 ACP 推回桌面。",
                    "AssistantDelta + tool_calls",
                    &["xai-grok-sampler", "xai-grok-sampling-types"],
                    &[
                        cref(
                            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs",
                            "run_turn_via_sampler",
                            "一轮模型调用",
                        ),
                        cref(
                            "crates/codegen/bony-build/src/model.rs",
                            "AppModel::apply",
                            "流式事件归约到时间线",
                        ),
                    ],
                ),
                step(
                    5,
                    "工具",
                    "执行工具（如有）",
                    "ToolBridge",
                    "改文件/跑命令在 Workspace 权限与沙箱内落地；结果回灌后再采样。",
                    "tool result → next messages",
                    &["xai-grok-tools", "xai-tool-runtime", "xai-grok-workspace"],
                    &[
                        cref(
                            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs",
                            "execute_tool_calls",
                            "一批工具执行与权限门",
                        ),
                        cref(
                            "crates/codegen/xai-grok-tools",
                            "FinalizedToolset::call",
                            "具体工具实现入口",
                        ),
                    ],
                ),
                step(
                    6,
                    "落盘",
                    "轮次结束并记账",
                    "desktop + storage",
                    "stop_reason / token usage 回 UI；追加 turns.jsonl，用量面板画趋势。",
                    "~/.bony-build/turns.jsonl",
                    &["bony-build"],
                    &[
                        cref(
                            "crates/codegen/bony-build/src/usage.rs",
                            "turns.jsonl",
                            "token 用量持久化",
                        ),
                        cref(
                            "crates/codegen/bony-build/src/charts.rs",
                            "usage charts",
                            "折线 / 柱状趋势",
                        ),
                        cref(
                            "crates/codegen/bony-build/src/markdown.rs",
                            "render markdown",
                            "助手消息轻量渲染",
                        ),
                    ],
                ),
            ],
        },
        WorkflowScene {
            id: "turn".into(),
            title: "分镜二 · Agent turn 怎么转".into(),
            summary: "Agent 只负责装配；SessionActor 才跑 loop。旁路含 memory / compaction / subagent。".into(),
            image: Some("/repo-docs/architecture-turn-flow.png".into()),
            image_caption: Some("Turn 流程：采样 → 工具 → 再采样".into()),
            image_side: "left".into(),
            chart_ids: vec!["call-turn".into()],
            steps: vec![
                step(
                    1,
                    "装配",
                    "Agent 已装配好",
                    "xai-grok-agent",
                    "AgentBuilder 造出带提示词与工具的 Agent；不负责跑 loop。",
                    "Agent + ToolBridge",
                    &["xai-grok-agent"],
                    &[
                        cref(
                            "crates/codegen/xai-grok-agent/src/builder.rs",
                            "AgentBuilder::build",
                            "skills / tools / prompt render",
                        ),
                        cref(
                            "crates/codegen/xai-grok-agent/src/agent.rs",
                            "struct Agent",
                            "不可跨 session 移植的绑定对象",
                        ),
                    ],
                ),
                step(
                    2,
                    "循环",
                    "SessionActor 跑 loop",
                    "xai-grok-shell",
                    "真正的 agentic turn：采样 → 工具 → 再采样，旁路含 memory / compaction。",
                    "turn events via ACP",
                    &["xai-grok-shell"],
                    &[
                        cref(
                            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/run_loop.rs",
                            "run_session",
                            "命令 / 事件 / idle memory flush",
                        ),
                        cref(
                            "crates/codegen/xai-chat-state",
                            "ChatStateHandle::build_request",
                            "组装采样请求",
                        ),
                    ],
                ),
            ],
        },
        WorkflowScene {
            id: "layers".into(),
            title: "分镜三 · 分层与 crate 地图".into(),
            summary: "Host → Session → Agent / Sampler / Tools → Workspace；下方条形图来自实时扫描。".into(),
            image: Some("/repo-docs/architecture-layers.png".into()),
            image_caption: Some("分层架构总览（与 ARCHITECTURE.md 同源）".into()),
            image_side: "right".into(),
            chart_ids: vec!["layers-stack".into(), "bars-modules".into()],
            steps: vec![
                step(
                    1,
                    "Host",
                    "Host / 客户端",
                    "bony-build / TUI",
                    "只负责界面与 ACP；不内嵌完整 agent 运行时。",
                    "desktop or pager UI",
                    &["bony-build", "xai-grok-pager"],
                    &[cref(
                        "crates/codegen/bony-build/src/main.rs",
                        "eframe::run_native",
                        "桌面进程入口",
                    )],
                ),
                step(
                    2,
                    "Runtime",
                    "Session + 三件套",
                    "shell / sampler / tools",
                    "Session 托管 loop；采样打模型；工具改工作区。",
                    "stdio agent process",
                    &["xai-grok-shell", "xai-grok-sampler", "xai-grok-tools"],
                    &[],
                ),
                step(
                    3,
                    "WS",
                    "Workspace 落地",
                    "workspace / sandbox",
                    "权限、沙箱、文件系统、checkpoint 约束工具副作用。",
                    "FS + VCS + permissions",
                    &["xai-grok-workspace", "xai-grok-sandbox"],
                    &[],
                ),
            ],
        },
        WorkflowScene {
            id: "switch".into(),
            title: "分镜四 · 换项目 / 换模型 / 鉴权".into(),
            summary: "cwd 与默认模型变更会重建 bridge；BYOK / cached token / grok.com 决定是否弹登录。".into(),
            image: Some("/repo-docs/bony-build-desktop.png".into()),
            image_caption: Some("侧栏项目与底部模型选择器".into()),
            image_side: "left".into(),
            chart_ids: vec!["seq-auth".into()],
            steps: vec![
                step(
                    1,
                    "换目录",
                    "切换工作目录",
                    "Bony Build",
                    "Shutdown 旧 agent → 新 cwd 再 spawn grok agent stdio → session/new。",
                    "new session_id",
                    &["bony-build"],
                    &[cref(
                        "crates/codegen/bony-build/src/app.rs",
                        "switch_project",
                        "清 timeline 并重建 bridge",
                    )],
                ),
                step(
                    2,
                    "换模型",
                    "切换默认模型",
                    "model picker",
                    "ACP session/set_model，并写回 ~/.grok/config.toml 的 [models] default。",
                    "config.toml default",
                    &["bony-build", "xai-grok-shell"],
                    &[cref(
                        "crates/codegen/bony-build/src/config_io.rs",
                        "set_default_model",
                        "持久化默认模型",
                    )],
                ),
                step(
                    3,
                    "鉴权",
                    "凭证解析顺序",
                    "auth + BYOK",
                    "hydrate env → auth.json / XAI_API_KEY / config env_key → 否则 NeedsLogin。",
                    "authMethods on initialize",
                    &["bony-build", "xai-grok-shell"],
                    &[cref(
                        "crates/codegen/bony-build/src/agent_bridge.rs",
                        "initialize / authenticate",
                        "跳过强制浏览器登录（有 BYOK 时）",
                    )],
                ),
            ],
        },
        WorkflowScene {
            id: "monitor".into(),
            title: "分镜五 · 监控看板接到哪里".into(),
            summary: "bony-monitor 不跑 agent；读 git + features.toml + 源码扫描，生成影响摘要。".into(),
            image: None,
            image_caption: None,
            image_side: "right".into(),
            chart_ids: vec!["flow-monitor".into()],
            steps: vec![
                step(
                    1,
                    "扫描",
                    "读 git + 功能目录",
                    "bony-monitor",
                    "扫描近期 commit，用 features.toml 匹配路径与关键词；热重载 catalog。",
                    "/api/changes · /api/features · /api/workflow",
                    &["bony-monitor"],
                    &[
                        cref(
                            "crates/codegen/bony-monitor/src/git.rs",
                            "list_changes",
                            "git log --numstat",
                        ),
                        cref(
                            "crates/codegen/bony-monitor/catalog/features.toml",
                            "[[features]]",
                            "人工功能规则",
                        ),
                        cref(
                            "crates/codegen/bony-monitor/src/catalog.rs",
                            "CatalogCache",
                            "toml + 模块扫描热更新",
                        ),
                    ],
                ),
                step(
                    2,
                    "影响",
                    "生成影响摘要",
                    "impact engine",
                    "标出产品功能、风险维度与建议回归清单。",
                    "timeline + drawer",
                    &["bony-monitor"],
                    &[
                        cref(
                            "crates/codegen/bony-monitor/src/impact.rs",
                            "analyze_change",
                            "路径 / 文案 → 区域与功能",
                        ),
                        cref(
                            "crates/codegen/bony-monitor/src/features.rs",
                            "features_overview",
                            "功能热力矩阵",
                        ),
                    ],
                ),
            ],
        },
    ]
}

fn gallery() -> Vec<WorkflowGalleryItem> {
    vec![
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
    ]
}

fn build_charts(catalog: &CatalogSnapshot, code_map: &[CodeModule]) -> Vec<WorkflowChart> {
    let mut by_crate: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for d in &catalog.discovered {
        *by_crate.entry(d.crate_name.clone()).or_default() += 1;
    }
    // Include curated runtime crates even if not in discovered scan.
    for c in [
        "xai-grok-shell",
        "xai-grok-agent",
        "xai-grok-sampler",
        "xai-grok-tools",
        "xai-grok-workspace",
        "bony-monitor",
    ] {
        by_crate.entry(c.into()).or_insert(0);
    }
    let bars: Vec<BarItem> = {
        let mut v: Vec<_> = by_crate
            .into_iter()
            .map(|(label, value)| {
                let note = code_map
                    .iter()
                    .find(|m| m.crate_name == label)
                    .map(|m| m.layer.clone())
                    .unwrap_or_else(|| "runtime".into());
                BarItem { label, value, note }
            })
            .collect();
        v.sort_by(|a, b| b.value.cmp(&a.value).then(a.label.cmp(&b.label)));
        v.into_iter().take(12).collect()
    };

    vec![
        WorkflowChart {
            id: "seq-prompt".into(),
            title: "时序 · 一次 Prompt".into(),
            caption: "从 UI 输入到 turns.jsonl 的跨进程调用".into(),
            kind: "sequence".into(),
            actors: vec![
                "User".into(),
                "app.rs".into(),
                "agent_bridge".into(),
                "SessionActor".into(),
                "Sampler".into(),
                "ToolBridge".into(),
                "usage.rs".into(),
            ],
            messages: vec![
                msg("User", "app.rs", "Enter prompt"),
                msg("app.rs", "agent_bridge", "UiCommand::Prompt"),
                msg("agent_bridge", "SessionActor", "ACP session/prompt"),
                msg("SessionActor", "Sampler", "build_request → SSE"),
                msg("Sampler", "SessionActor", "deltas / tool_calls"),
                msg("SessionActor", "ToolBridge", "execute_tool_calls"),
                msg("ToolBridge", "SessionActor", "tool results"),
                msg("SessionActor", "agent_bridge", "PromptResponse + updates"),
                msg("agent_bridge", "app.rs", "AgentEvent::*"),
                msg("app.rs", "usage.rs", "TurnDone → turns.jsonl"),
            ],
            bands: vec![],
            nodes: vec![],
            edges: vec![],
            bars: vec![],
        },
        WorkflowChart {
            id: "flow-prompt".into(),
            title: "数据流 · Prompt 管线".into(),
            caption: "关键类型沿管道传递".into(),
            kind: "flow".into(),
            actors: vec![],
            messages: vec![],
            bands: vec![],
            nodes: vec![
                node("ui", "UiCommand::Prompt", "app.rs"),
                node("acp", "PromptRequest", "agent_bridge + ACP"),
                node("turn", "handle_prompt", "xai-grok-shell"),
                node("sample", "SamplerTurn", "xai-grok-sampler"),
                node("tools", "ToolLoop", "tools + workspace"),
                node("ui2", "AgentEvent", "model.rs apply"),
                node("disk", "turns.jsonl", "usage.rs"),
            ],
            edges: vec![
                edge("ui", "acp", "channel"),
                edge("acp", "turn", "stdio JSON-RPC"),
                edge("turn", "sample", "ChatState"),
                edge("sample", "tools", "if tool_calls"),
                edge("tools", "sample", "resubmit"),
                edge("sample", "ui2", "stream"),
                edge("ui2", "disk", "persist"),
            ],
            bars: vec![],
        },
        WorkflowChart {
            id: "call-turn".into(),
            title: "调用图 · Turn 内部".into(),
            caption: "装配与 loop 的职责边界".into(),
            kind: "callgraph".into(),
            actors: vec![],
            messages: vec![],
            bands: vec![],
            nodes: vec![
                node("builder", "AgentBuilder", "xai-grok-agent"),
                node("agent", "Agent", "prompt + ToolBridge"),
                node("session", "SessionActor", "owns loop"),
                node("chat", "ChatState", "messages"),
                node("sampler", "SamplerHandle", "HTTP/SSE"),
                node("tools", "ToolBridge", "execute"),
                node("ws", "Workspace", "FS / perms"),
                node("mem", "memory / compact", "旁路"),
            ],
            edges: vec![
                edge("builder", "agent", "build()"),
                edge("session", "agent", "holds"),
                edge("session", "chat", "build_request"),
                edge("session", "sampler", "submit"),
                edge("session", "tools", "tool_calls"),
                edge("tools", "ws", "side effects"),
                edge("session", "mem", "hooks"),
                edge("sampler", "session", "outcome"),
                edge("tools", "session", "results"),
            ],
            bars: vec![],
        },
        WorkflowChart {
            id: "layers-stack".into(),
            title: "分层 · Host → Workspace".into(),
            caption: "与 ARCHITECTURE.md 对齐的可读栈".into(),
            kind: "layers".into(),
            actors: vec![],
            messages: vec![],
            bands: vec![
                LayerBand {
                    name: "1 · Host".into(),
                    summary: "界面与 ACP 客户端".into(),
                    items: vec![
                        "bony-build (egui)".into(),
                        "xai-grok-pager (TUI)".into(),
                        "bony-monitor (dashboard)".into(),
                    ],
                },
                LayerBand {
                    name: "2 · Session".into(),
                    summary: "agentic turn 宿主".into(),
                    items: vec!["xai-grok-shell · SessionActor".into(), "xai-chat-state".into()],
                },
                LayerBand {
                    name: "3a · Agent".into(),
                    summary: "定义与提示词（不跑 loop）".into(),
                    items: vec!["xai-grok-agent · AgentBuilder".into()],
                },
                LayerBand {
                    name: "3b · Sampling".into(),
                    summary: "模型 HTTP/SSE".into(),
                    items: vec!["xai-grok-sampler".into(), "xai-grok-sampling-types".into()],
                },
                LayerBand {
                    name: "3c · Tools".into(),
                    summary: "tool call 落地".into(),
                    items: vec![
                        "xai-grok-tools".into(),
                        "xai-tool-runtime".into(),
                        "xai-tool-protocol".into(),
                    ],
                },
                LayerBand {
                    name: "4 · Workspace".into(),
                    summary: "权限 / 沙箱 / FS / checkpoint".into(),
                    items: vec!["xai-grok-workspace".into(), "xai-grok-sandbox".into()],
                },
            ],
            nodes: vec![],
            edges: vec![],
            bars: vec![],
        },
        WorkflowChart {
            id: "bars-modules".into(),
            title: "模块扫描 · 按 crate 文件数".into(),
            caption: format!(
                "当前扫描 {} 个源文件（桌面模块 {}）",
                catalog.discovered.len(),
                catalog.desktop_module_count
            ),
            kind: "bars".into(),
            actors: vec![],
            messages: vec![],
            bands: vec![],
            nodes: vec![],
            edges: vec![],
            bars,
        },
        WorkflowChart {
            id: "seq-auth".into(),
            title: "时序 · 启动与鉴权".into(),
            caption: "BYOK 优先，避免无密钥时误弹浏览器登录".into(),
            kind: "sequence".into(),
            actors: vec![
                "main.rs".into(),
                "config_io".into(),
                "agent_bridge".into(),
                "grok agent".into(),
                "AppModel".into(),
            ],
            messages: vec![
                msg("main.rs", "config_io", "load config.toml + catalog"),
                msg("config_io", "agent_bridge", "hydrate_model_env_keys"),
                msg("agent_bridge", "grok agent", "spawn stdio + initialize"),
                msg("grok agent", "agent_bridge", "authMethods"),
                msg("agent_bridge", "grok agent", "authenticate (if needed)"),
                msg("agent_bridge", "grok agent", "session/new ± modelId"),
                msg("agent_bridge", "AppModel", "AgentEvent::Connected"),
            ],
            bands: vec![],
            nodes: vec![],
            edges: vec![],
            bars: vec![],
        },
        WorkflowChart {
            id: "flow-monitor".into(),
            title: "数据流 · bony-monitor".into(),
            caption: "只读解释层，不参与 agent turn".into(),
            kind: "flow".into(),
            actors: vec![],
            messages: vec![],
            bands: vec![],
            nodes: vec![
                node("git", "git log", "git.rs"),
                node("toml", "features.toml", "catalog"),
                node("scan", "src scan", "discovered.json"),
                node("impact", "impact.rs", "match features"),
                node("api", "/api/*", "Axum"),
                node("ui", "static SPA", "app.js"),
            ],
            edges: vec![
                edge("git", "impact", "ChangeEntry"),
                edge("toml", "impact", "rules"),
                edge("scan", "api", "module counts"),
                edge("impact", "api", "JSON"),
                edge("api", "ui", "fetch"),
            ],
            bars: vec![],
        },
    ]
}

fn build_code_map(catalog: &CatalogSnapshot) -> Vec<CodeModule> {
    let curated = curated_modules();
    let mut out = curated.clone();
    let known: std::collections::HashSet<String> =
        curated.iter().map(|m| m.path.clone()).collect();

    for d in &catalog.discovered {
        if known.contains(&d.path) {
            continue;
        }
        out.push(module_from_discovered(d));
    }

    out.sort_by(|a, b| {
        a.layer
            .cmp(&b.layer)
            .then(a.crate_name.cmp(&b.crate_name))
            .then(a.path.cmp(&b.path))
    });
    out
}

fn curated_modules() -> Vec<CodeModule> {
    vec![
        modu(
            "crates/codegen/bony-build/src/main.rs",
            "main",
            "bony-build",
            "Host",
            "eframe 入口与 BridgeConfig",
            &["main"],
        ),
        modu(
            "crates/codegen/bony-build/src/app.rs",
            "app",
            "bony-build",
            "Host",
            "UI 布局、发 Prompt、换项目、消费事件",
            &["BonyBuildApp", "UiCommand"],
        ),
        modu(
            "crates/codegen/bony-build/src/agent_bridge.rs",
            "agent_bridge",
            "bony-build",
            "Host",
            "spawn grok agent stdio、ACP 会话与鉴权",
            &["spawn_bridge", "PromptRequest"],
        ),
        modu(
            "crates/codegen/bony-build/src/events.rs",
            "events",
            "bony-build",
            "Host",
            "UI ↔ bridge 消息枚举",
            &["UiCommand", "AgentEvent"],
        ),
        modu(
            "crates/codegen/bony-build/src/model.rs",
            "model",
            "bony-build",
            "Host",
            "AppModel 时间线与事件归约",
            &["AppModel"],
        ),
        modu(
            "crates/codegen/bony-build/src/config_io.rs",
            "config_io",
            "bony-build",
            "Host",
            "config.toml 模型目录、default、env hydrate",
            &["hydrate_model_env_keys"],
        ),
        modu(
            "crates/codegen/bony-build/src/usage.rs",
            "usage",
            "bony-build",
            "Host",
            "turns.jsonl 用量持久化",
            &["TurnRecord"],
        ),
        modu(
            "crates/codegen/bony-build/src/charts.rs",
            "charts",
            "bony-build",
            "Host",
            "用量折线 / 柱状图",
            &[],
        ),
        modu(
            "crates/codegen/bony-build/src/markdown.rs",
            "markdown",
            "bony-build",
            "Host",
            "助手消息轻量 markdown",
            &[],
        ),
        modu(
            "crates/codegen/xai-grok-shell/src/session/acp_session.rs",
            "acp_session",
            "xai-grok-shell",
            "Session",
            "SessionActor 宿主",
            &["SessionActor", "MvpAgent"],
        ),
        modu(
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/turn.rs",
            "turn",
            "xai-grok-shell",
            "Session",
            "handle_prompt 单 turn",
            &["handle_prompt"],
        ),
        modu(
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs",
            "sampler_turn",
            "xai-grok-shell",
            "Session",
            "一轮采样调用",
            &["run_turn_via_sampler"],
        ),
        modu(
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs",
            "tool_calls",
            "xai-grok-shell",
            "Session",
            "工具批处理与权限",
            &["execute_tool_calls"],
        ),
        modu(
            "crates/codegen/xai-grok-agent/src/builder.rs",
            "builder",
            "xai-grok-agent",
            "Agent",
            "AgentBuilder 装配管线",
            &["AgentBuilder::build"],
        ),
        modu(
            "crates/codegen/xai-grok-agent/src/agent.rs",
            "agent",
            "xai-grok-agent",
            "Agent",
            "已装配 Agent 结构体",
            &["Agent"],
        ),
        modu(
            "crates/codegen/xai-grok-sampler",
            "sampler",
            "xai-grok-sampler",
            "Sampling",
            "多 backend HTTP/SSE",
            &["SamplerHandle"],
        ),
        modu(
            "crates/codegen/xai-grok-tools",
            "tools",
            "xai-grok-tools",
            "Tools",
            "终端 / 编辑 / 搜索等工具",
            &["ToolBridge", "FinalizedToolset"],
        ),
        modu(
            "crates/codegen/xai-grok-workspace",
            "workspace",
            "xai-grok-workspace",
            "Workspace",
            "FS、VCS、权限、checkpoint",
            &["PermissionManager"],
        ),
        modu(
            "crates/codegen/bony-monitor/src/main.rs",
            "main",
            "bony-monitor",
            "Monitor",
            "Axum 路由与静态页",
            &["/api/workflow"],
        ),
        modu(
            "crates/codegen/bony-monitor/src/workflow.rs",
            "workflow",
            "bony-monitor",
            "Monitor",
            "本页图表与分镜数据",
            &["overview"],
        ),
        modu(
            "crates/codegen/bony-monitor/src/impact.rs",
            "impact",
            "bony-monitor",
            "Monitor",
            "改动 → 功能影响",
            &["analyze_change"],
        ),
        modu(
            "crates/codegen/bony-monitor/src/catalog.rs",
            "catalog",
            "bony-monitor",
            "Monitor",
            "features.toml 热重载 + 扫描",
            &["CatalogCache"],
        ),
    ]
}

fn module_from_discovered(d: &DiscoveredFile) -> CodeModule {
    let layer = match d.crate_name.as_str() {
        "bony-build" => "Host",
        "bony-monitor" => "Monitor",
        _ => "Discovered",
    };
    CodeModule {
        path: d.path.clone(),
        stem: d.stem.clone(),
        crate_name: d.crate_name.clone(),
        layer: layer.into(),
        role: format!("扫描到的 {} 模块", d.stem),
        key_types: vec![],
    }
}

fn modu(
    path: &str,
    stem: &str,
    crate_name: &str,
    layer: &str,
    role: &str,
    key_types: &[&str],
) -> CodeModule {
    CodeModule {
        path: path.into(),
        stem: stem.into(),
        crate_name: crate_name.into(),
        layer: layer.into(),
        role: role.into(),
        key_types: key_types.iter().map(|s| (*s).into()).collect(),
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
    code_refs: &[CodeRef],
) -> WorkflowStep {
    WorkflowStep {
        n,
        chip: chip.into(),
        title: title.into(),
        actor: actor.into(),
        action: action.into(),
        artifact: artifact.into(),
        crates: crates.iter().map(|s| (*s).into()).collect(),
        code_refs: code_refs.to_vec(),
    }
}

fn cref(path: &str, symbol: &str, note: &str) -> CodeRef {
    CodeRef {
        path: path.into(),
        symbol: symbol.into(),
        note: note.into(),
    }
}

fn msg(from: &str, to: &str, label: &str) -> SeqMessage {
    SeqMessage {
        from: from.into(),
        to: to.into(),
        label: label.into(),
    }
}

fn node(id: &str, label: &str, detail: &str) -> FlowNode {
    FlowNode {
        id: id.into(),
        label: label.into(),
        detail: detail.into(),
    }
}

fn edge(from: &str, to: &str, label: &str) -> FlowEdge {
    FlowEdge {
        from: from.into(),
        to: to.into(),
        label: label.into(),
    }
}
