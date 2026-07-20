//! Map changed paths / commit text into areas, product features, and dimensions.

use serde::Serialize;

use crate::catalog::CatalogSnapshot;
use crate::features::{self, DimensionHit, FeatureHit};

#[derive(Debug, Clone, Serialize)]
pub struct ImpactArea {
    pub id: String,
    pub label: String,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeImpact {
    /// Coarse module areas (desktop / runtime / docs …).
    pub areas: Vec<ImpactArea>,
    /// Product features affected (chat, models, tools …).
    pub features: Vec<FeatureHit>,
    /// Cross-cutting dimensions (UX, security, reliability …).
    pub dimensions: Vec<DimensionHit>,
    /// Suggested verification checklist.
    pub checklist: Vec<String>,
    pub improvements: Vec<String>,
    pub risks: Vec<String>,
    pub tags: Vec<String>,
}

struct Rule {
    id: &'static str,
    label: &'static str,
    prefixes: &'static [&'static str],
    severity: &'static str,
    improvement: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        id: "desktop",
        label: "桌面客户端",
        prefixes: &["crates/codegen/bony-build/", "scripts/run-desktop.ps1"],
        severity: "high",
        improvement: "改进桌面壳、对话、用量统计或项目工作区入口",
    },
    Rule {
        id: "monitor",
        label: "Web 监控",
        prefixes: &["crates/codegen/bony-monitor/", "scripts/run-monitor.ps1"],
        severity: "medium",
        improvement: "增强架构与改动可观测性",
    },
    Rule {
        id: "agent-runtime",
        label: "Agent 运行时",
        prefixes: &[
            "crates/codegen/xai-grok-shell/",
            "crates/codegen/xai-grok-agent/",
            "crates/codegen/xai-grok-tools/",
            "crates/codegen/xai-grok-sampler/",
            "crates/codegen/xai-acp-lib/",
        ],
        severity: "high",
        improvement: "影响会话循环、工具执行或 ACP 协议行为",
    },
    Rule {
        id: "tui",
        label: "终端 TUI",
        prefixes: &[
            "crates/codegen/xai-grok-pager/",
            "crates/codegen/xai-grok-pager-bin/",
            "crates/codegen/xai-grok-pager-render/",
        ],
        severity: "medium",
        improvement: "影响终端界面或官方 grok CLI 体验",
    },
    Rule {
        id: "docs",
        label: "文档与架构",
        prefixes: &[
            "ARCHITECTURE.md",
            "README.md",
            "docs/",
            "crates/codegen/xai-grok-pager/docs/",
        ],
        severity: "low",
        improvement: "完善产品说明、架构图或使用文档",
    },
    Rule {
        id: "workspace",
        label: "构建 / Workspace",
        prefixes: &[
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "scripts/",
        ],
        severity: "medium",
        improvement: "调整依赖、工作区成员或开发脚本",
    },
    Rule {
        id: "config",
        label: "配置与认证",
        prefixes: &[
            "crates/codegen/xai-grok-config/",
            "crates/codegen/xai-grok-auth/",
            "crates/codegen/xai-grok-secrets/",
        ],
        severity: "high",
        improvement: "影响模型配置、认证或密钥处理",
    },
];

pub fn analyze(
    catalog: &CatalogSnapshot,
    subject: &str,
    body: &str,
    files: &[String],
) -> ChangeImpact {
    let mut areas = Vec::new();
    let mut tags = Vec::new();
    let mut improvements = Vec::new();
    let mut risks = Vec::new();

    for rule in RULES {
        let hits: Vec<&String> = files
            .iter()
            .filter(|f| rule.prefixes.iter().any(|p| f.starts_with(p) || *f == *p))
            .collect();
        if hits.is_empty() {
            continue;
        }
        tags.push(rule.id.to_string());
        areas.push(ImpactArea {
            id: rule.id.to_string(),
            label: rule.label.to_string(),
            severity: rule.severity.to_string(),
            summary: format!("{}（{} 个文件）", rule.improvement, hits.len()),
        });
        improvements.push(rule.improvement.to_string());
    }

    if areas.is_empty() && !files.is_empty() {
        areas.push(ImpactArea {
            id: "other".into(),
            label: "其他改动".into(),
            severity: "low".into(),
            summary: format!("触及 {} 个文件", files.len()),
        });
        tags.push("other".into());
    }

    for line in body.lines().chain(subject.lines()) {
        let t = line.trim();
        if let Some(rest) = strip_prefix_ci(t, "Impact:").or_else(|| strip_prefix_ci(t, "影响:"))
        {
            let rest = rest.trim();
            if !rest.is_empty() {
                improvements.push(rest.to_string());
            }
        }
        if let Some(rest) =
            strip_prefix_ci(t, "Improvement:").or_else(|| strip_prefix_ci(t, "改进:"))
        {
            let rest = rest.trim();
            if !rest.is_empty() {
                improvements.push(rest.to_string());
            }
        }
        if let Some(rest) = strip_prefix_ci(t, "Risk:").or_else(|| strip_prefix_ci(t, "风险:")) {
            let rest = rest.trim();
            if !rest.is_empty() {
                risks.push(rest.to_string());
            }
        }
    }

    let joined = files.join("\n");
    if joined.contains("auth") || joined.contains("secret") {
        risks.push("可能影响认证或密钥路径，部署前请回归登录流程".into());
    }
    if files
        .iter()
        .any(|f| f.ends_with("Cargo.lock") || f == "Cargo.toml")
    {
        risks.push("依赖变更可能影响构建复现性，建议本地 cargo check".into());
    }

    let (feature_hits, dimensions, checklist) =
        features::analyze_features_full(catalog, subject, body, files);

    for f in &feature_hits {
        if !tags.iter().any(|t| t == &f.id) {
            tags.push(f.id.clone());
        }
        improvements.push(format!("【功能·{}】{}", f.name, f.user_impact));
        if f.severity == "critical" || f.severity == "high" {
            risks.push(format!("回归「{}」—— {}", f.name, f.why));
        }
    }
    for d in &dimensions {
        if d.level == "高" {
            risks.push(format!("【维度·{}】{}", d.label, d.note));
        }
    }

    improvements.sort();
    improvements.dedup();
    risks.sort();
    risks.dedup();

    ChangeImpact {
        areas,
        features: feature_hits,
        dimensions,
        checklist,
        improvements,
        risks,
        tags,
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::strip_prefix_ci;

    #[test]
    fn strip_prefix_is_safe_for_utf8_input() {
        assert_eq!(strip_prefix_ci("当前未暂存修改", "Impact:"), None);
        assert_eq!(
            strip_prefix_ci("影响: 会话恢复", "影响:"),
            Some(" 会话恢复")
        );
    }
}
