//! Product feature catalog — maps file changes to end-user capabilities.

use serde::Serialize;

use crate::git::ChangeEntry;

#[derive(Debug, Clone, Serialize)]
pub struct FeatureDef {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub user_facing: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureHit {
    pub id: String,
    pub name: String,
    pub category: String,
    pub severity: String,
    pub why: String,
    pub user_impact: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DimensionHit {
    pub id: String,
    pub label: String,
    pub level: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureActivity {
    pub feature: FeatureDef,
    pub commit_count: u64,
    pub additions: u64,
    pub deletions: u64,
    pub last_sha: String,
    pub last_subject: String,
    pub last_date: String,
    pub heat: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeaturesOverview {
    pub catalog: Vec<FeatureDef>,
    pub activity: Vec<FeatureActivity>,
    pub dimensions: Vec<&'static str>,
}

struct FeatureRule {
    id: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
    user_facing: bool,
    /// Path prefix / contains matchers.
    paths: &'static [&'static str],
    /// Filename fragment matchers.
    names: &'static [&'static str],
    /// Subject/body keywords (lowercase).
    keywords: &'static [&'static str],
    severity: &'static str,
    user_impact: &'static str,
}

const FEATURES: &[FeatureRule] = &[
    FeatureRule {
        id: "chat",
        name: "对话交互",
        category: "桌面体验",
        description: "消息发送、流式回复、时间线、快捷键",
        user_facing: true,
        paths: &["crates/codegen/bony-build/src/app.rs", "crates/codegen/bony-build/src/model.rs", "crates/codegen/bony-build/src/markdown.rs"],
        names: &["app.rs", "markdown.rs"],
        keywords: &["chat", "composer", "对话", "发送", "enter"],
        severity: "high",
        user_impact: "直接影响用户如何提问与阅读回复",
    },
    FeatureRule {
        id: "ui-layout",
        name: "界面布局 / 可读性",
        category: "桌面体验",
        description: "布局、对齐、字体、主题、空状态",
        user_facing: true,
        paths: &["crates/codegen/bony-build/src/fonts.rs", "crates/codegen/bony-build/src/app.rs"],
        names: &["fonts.rs"],
        keywords: &["layout", "font", "cjk", "中文", "乱码", "ui"],
        severity: "medium",
        user_impact: "影响界面是否易读、是否被遮挡、中文是否正常",
    },
    FeatureRule {
        id: "model-picker",
        name: "模型切换",
        category: "桌面体验",
        description: "会话模型选择、默认模型持久化",
        user_facing: true,
        paths: &["crates/codegen/bony-build/src/config_io.rs"],
        names: &["config_io.rs"],
        keywords: &["model", "set_session_model", "模型", "qwen", "kimi", "glm"],
        severity: "high",
        user_impact: "用户能否切换 Kimi/Qwen/智谱等模型并记住默认",
    },
    FeatureRule {
        id: "auth-login",
        name: "登录 / 认证",
        category: "安全与接入",
        description: "浏览器登录、API Key、BYOK 门闸",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-auth/",
            "crates/codegen/xai-grok-secrets/",
            "crates/codegen/bony-build/src/agent_bridge.rs",
        ],
        names: &["auth", "login", "secret"],
        keywords: &["login", "auth", "api_key", "登录", "sign in", "byok"],
        severity: "critical",
        user_impact: "决定能否开始对话；凭证错误会阻断全部能力",
    },
    FeatureRule {
        id: "multi-provider",
        name: "多 LLM 供应商",
        category: "模型能力",
        description: "OpenAI 兼容 / Anthropic / 自定义 base_url",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-sampler/",
            "crates/codegen/xai-grok-sampling-types/",
            "crates/codegen/xai-grok-models/",
            "crates/codegen/xai-grok-config/",
        ],
        names: &["sampling", "model"],
        keywords: &["provider", "base_url", "dashscope", "moonshot", "zhipu", "供应商"],
        severity: "high",
        user_impact: "影响可用模型目录、请求协议与出站鉴权方式",
    },
    FeatureRule {
        id: "tools",
        name: "工具执行",
        category: "Agent 能力",
        description: "终端、文件编辑、搜索等 tool calls",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-tools/",
            "crates/codegen/xai-tool-runtime/",
            "crates/codegen/xai-tool-protocol/",
        ],
        names: &["tool"],
        keywords: &["tool", "shell", "edit", "工具"],
        severity: "high",
        user_impact: "Agent 能否真正改文件、跑命令、探索代码库",
    },
    FeatureRule {
        id: "permissions",
        name: "权限审批",
        category: "安全与接入",
        description: "工具权限弹窗、自动批准策略",
        user_facing: true,
        paths: &["crates/codegen/bony-build/src/app.rs", "crates/codegen/xai-grok-workspace/"],
        names: &["permission"],
        keywords: &["permission", "approve", "yolo", "权限", "批准"],
        severity: "high",
        user_impact: "影响危险操作是否需确认，关系到安全与流畅度",
    },
    FeatureRule {
        id: "session-loop",
        name: "会话循环 / ACP",
        category: "Agent 能力",
        description: "prompt 循环、取消、stdio ACP 桥接",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-shell/",
            "crates/codegen/xai-acp-lib/",
            "crates/codegen/bony-build/src/agent_bridge.rs",
            "crates/codegen/xai-chat-state/",
        ],
        names: &["agent_bridge", "session", "acp"],
        keywords: &["session", "acp", "prompt", "cancel", "会话"],
        severity: "critical",
        user_impact: "核心链路；异常会导致无法对话或中途卡住",
    },
    FeatureRule {
        id: "workspace-fs",
        name: "工作区 / 文件系统",
        category: "Agent 能力",
        description: "cwd、读写、checkpoint、沙箱",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-workspace/",
            "crates/codegen/xai-grok-sandbox/",
            "crates/codegen/xai-file-utils/",
        ],
        names: &["workspace", "sandbox", "checkpoint"],
        keywords: &["workspace", "cwd", "sandbox", "工作区"],
        severity: "high",
        user_impact: "决定 Agent 能在哪些目录操作、是否可回滚",
    },
    FeatureRule {
        id: "memory",
        name: "记忆 / 压缩",
        category: "Agent 能力",
        description: "长期记忆、上下文 compact",
        user_facing: false,
        paths: &[
            "crates/codegen/xai-grok-memory/",
            "crates/codegen/xai-grok-compaction/",
        ],
        names: &["memory", "compact"],
        keywords: &["memory", "compaction", "记忆", "压缩"],
        severity: "medium",
        user_impact: "长会话是否丢上下文、跨会话是否记得偏好",
    },
    FeatureRule {
        id: "subagent",
        name: "子 Agent",
        category: "Agent 能力",
        description: "并行子任务与覆盖解析",
        user_facing: false,
        paths: &["crates/codegen/xai-grok-subagent-resolution/"],
        names: &["subagent"],
        keywords: &["subagent", "子 agent"],
        severity: "medium",
        user_impact: "复杂任务拆分与并行执行能力",
    },
    FeatureRule {
        id: "tui",
        name: "终端 TUI",
        category: "终端体验",
        description: "官方 grok 全屏终端界面",
        user_facing: true,
        paths: &[
            "crates/codegen/xai-grok-pager/",
            "crates/codegen/xai-grok-pager-bin/",
            "crates/codegen/xai-grok-pager-render/",
        ],
        names: &["pager"],
        keywords: &["tui", "pager", "终端"],
        severity: "medium",
        user_impact: "不用桌面时，终端体验是否可用",
    },
    FeatureRule {
        id: "monitor",
        name: "Web 监控看板",
        category: "可观测性",
        description: "架构总览与改动影响展示",
        user_facing: true,
        paths: &["crates/codegen/bony-monitor/", "scripts/run-monitor.ps1"],
        names: &["bony-monitor", "run-monitor"],
        keywords: &["monitor", "监控", "impact"],
        severity: "medium",
        user_impact: "团队能否看清架构与每次改动的功能影响",
    },
    FeatureRule {
        id: "docs-product",
        name: "产品文档",
        category: "文档",
        description: "README、架构说明、截图",
        user_facing: true,
        paths: &["README.md", "ARCHITECTURE.md", "docs/"],
        names: &["README", "ARCHITECTURE"],
        keywords: &["readme", "docs", "文档"],
        severity: "low",
        user_impact: "新用户能否快速理解并上手项目",
    },
    FeatureRule {
        id: "build-ci",
        name: "构建 / 依赖",
        category: "工程",
        description: "Cargo workspace、脚本、toolchain",
        user_facing: false,
        paths: &["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "scripts/"],
        names: &["Cargo.toml", "Cargo.lock"],
        keywords: &["cargo", "build", "依赖", "workspace"],
        severity: "medium",
        user_impact: "影响能否编译运行；间接阻塞所有功能交付",
    },
    FeatureRule {
        id: "mcp-hooks",
        name: "MCP / Hooks / 插件",
        category: "扩展",
        description: "外部工具协议与钩子",
        user_facing: false,
        paths: &[
            "crates/codegen/xai-grok-mcp/",
            "crates/codegen/xai-grok-hooks/",
            "crates/codegen/xai-grok-plugin-marketplace/",
        ],
        names: &["mcp", "hook", "plugin"],
        keywords: &["mcp", "hook", "plugin", "插件"],
        severity: "medium",
        user_impact: "扩展能力与第三方集成是否可用",
    },
];

const DIMENSIONS: &[(&str, &str)] = &[
    ("ux", "用户体验"),
    ("capability", "功能能力"),
    ("security", "安全合规"),
    ("reliability", "稳定性"),
    ("compat", "兼容性"),
    ("perf", "性能"),
    ("dx", "开发体验"),
    ("docs", "文档完备"),
];

pub fn catalog() -> Vec<FeatureDef> {
    FEATURES
        .iter()
        .map(|f| FeatureDef {
            id: f.id.into(),
            name: f.name.into(),
            category: f.category.into(),
            description: f.description.into(),
            user_facing: f.user_facing,
        })
        .collect()
}

pub fn match_features(subject: &str, body: &str, files: &[String]) -> Vec<FeatureHit> {
    let blob = format!("{subject}\n{body}").to_lowercase();
    let mut hits = Vec::new();

    for f in FEATURES {
        let mut reasons = Vec::new();
        let path_hits: Vec<&String> = files
            .iter()
            .filter(|path| {
                f.paths.iter().any(|p| path.starts_with(p) || *path == *p)
                    || f.names.iter().any(|n| {
                        path.to_ascii_lowercase().contains(&n.to_ascii_lowercase())
                    })
            })
            .collect();
        if !path_hits.is_empty() {
            reasons.push(format!("触及 {} 个相关文件", path_hits.len()));
        }
        let kw: Vec<&&str> = f
            .keywords
            .iter()
            .filter(|k| blob.contains(&k.to_lowercase()))
            .collect();
        if !kw.is_empty() {
            reasons.push(format!("提交说明命中关键词: {}", kw.iter().take(3).map(|s| **s).collect::<Vec<_>>().join(", ")));
        }
        if reasons.is_empty() {
            continue;
        }
        hits.push(FeatureHit {
            id: f.id.into(),
            name: f.name.into(),
            category: f.category.into(),
            severity: f.severity.into(),
            why: reasons.join("；"),
            user_impact: f.user_impact.into(),
        });
    }
    hits
}

pub fn match_dimensions(subject: &str, body: &str, files: &[String], features: &[FeatureHit]) -> Vec<DimensionHit> {
    let mut out = Vec::new();
    let joined = files.join("\n").to_lowercase();
    let text = format!("{subject}\n{body}").to_lowercase();

    let has_ui = features.iter().any(|f| f.category == "桌面体验" || f.id == "tui");
    let has_cap = features.iter().any(|f| {
        matches!(
            f.id.as_str(),
            "tools" | "session-loop" | "multi-provider" | "model-picker" | "workspace-fs"
        )
    });
    let has_sec = features.iter().any(|f| f.id == "auth-login" || f.id == "permissions")
        || joined.contains("auth")
        || joined.contains("secret");

    if has_ui {
        out.push(dim("ux", "高", "界面或交互路径被改动，需目视回归主流程"));
    }
    if has_cap {
        out.push(dim("capability", "高", "核心 Agent/模型/工具能力可能变化"));
    }
    if has_sec {
        out.push(dim("security", "高", "认证或权限相关，需重点验证密钥与审批"));
    }
    if features.iter().any(|f| f.id == "session-loop") {
        out.push(dim("reliability", "高", "会话主链路变更，关注中断、重连与取消"));
    } else if !features.is_empty() {
        out.push(dim("reliability", "中", "建议冒烟：启动 → 发一条消息 → 看工具是否正常"));
    }
    if features.iter().any(|f| f.id == "multi-provider" || f.id == "model-picker") {
        out.push(dim("compat", "高", "不同供应商协议/鉴权可能不兼容，建议多模型抽测"));
    }
    if text.contains("perf") || text.contains("性能") || text.contains("timeout") {
        out.push(dim("perf", "中", "提交提到性能/超时相关"));
    }
    if features.iter().any(|f| f.id == "build-ci" || f.id == "monitor") {
        out.push(dim("dx", "中", "构建脚本或监控变更，影响开发与发布效率"));
    }
    if features.iter().any(|f| f.id == "docs-product") {
        out.push(dim("docs", "低", "文档更新，利于上手但不改运行时行为"));
    }

    // Ensure we always expose dimension skeleton when there are file changes.
    if out.is_empty() && !files.is_empty() {
        out.push(dim("capability", "低", "未匹配到已知功能规则，请人工确认影响面"));
    }
    out
}

fn dim(id: &str, level: &str, note: &str) -> DimensionHit {
    let label = DIMENSIONS
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| id.to_string());
    DimensionHit {
        id: id.into(),
        label,
        level: level.into(),
        note: note.into(),
    }
}

pub fn analyze_features_full(
    subject: &str,
    body: &str,
    files: &[String],
) -> (Vec<FeatureHit>, Vec<DimensionHit>, Vec<String>) {
    let features = match_features(subject, body, files);
    let dimensions = match_dimensions(subject, body, files, &features);
    let mut checklist = Vec::new();
    for f in &features {
        checklist.push(format!("验证「{}」：{}", f.name, f.user_impact));
    }
    if features.iter().any(|f| f.id == "chat" || f.id == "session-loop") {
        checklist.push("冒烟：启动桌面 → 发送一条消息 → 确认流式回复".into());
    }
    if features.iter().any(|f| f.id == "model-picker" || f.id == "multi-provider") {
        checklist.push("冒烟：切换至少 2 个模型并各发一条短消息".into());
    }
    if features.iter().any(|f| f.id == "auth-login") {
        checklist.push("冒烟：无凭证提示登录 / 有 Key 可直连".into());
    }
    if features.iter().any(|f| f.id == "tools" || f.id == "permissions") {
        checklist.push("冒烟：触发一次读文件或列目录工具".into());
    }
    checklist.sort();
    checklist.dedup();
    (features, dimensions, checklist)
}

pub fn features_overview(changes: &[ChangeEntry]) -> FeaturesOverview {
    let mut activity = Vec::new();
    for f in FEATURES {
        let mut commit_count = 0u64;
        let mut additions = 0u64;
        let mut deletions = 0u64;
        let mut last_sha = String::new();
        let mut last_subject = String::new();
        let mut last_date = String::new();

        for c in changes {
            let hit = c.impact.features.iter().any(|h| h.id == f.id);
            if !hit {
                continue;
            }
            commit_count += 1;
            additions += u64::from(c.additions);
            deletions += u64::from(c.deletions);
            if last_sha.is_empty() {
                last_sha = c.short_sha.clone();
                last_subject = c.subject.clone();
                last_date = c.date.clone();
            }
        }

        let heat = if commit_count >= 3 {
            "hot"
        } else if commit_count >= 1 {
            "warm"
        } else {
            "idle"
        };

        activity.push(FeatureActivity {
            feature: FeatureDef {
                id: f.id.into(),
                name: f.name.into(),
                category: f.category.into(),
                description: f.description.into(),
                user_facing: f.user_facing,
            },
            commit_count,
            additions,
            deletions,
            last_sha,
            last_subject,
            last_date,
            heat: heat.into(),
        });
    }

    activity.sort_by(|a, b| {
        b.commit_count
            .cmp(&a.commit_count)
            .then(a.feature.name.cmp(&b.feature.name))
    });

    FeaturesOverview {
        catalog: catalog(),
        activity,
        dimensions: DIMENSIONS.iter().map(|(_, l)| *l).collect(),
    }
}
