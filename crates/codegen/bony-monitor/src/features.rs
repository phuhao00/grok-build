//! Product feature catalog — maps file changes to end-user capabilities.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::catalog::{CatalogSnapshot, rule_covers_path};
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
    pub dimensions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FeatureRule {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub user_facing: bool,
    pub paths: Vec<String>,
    pub names: Vec<String>,
    pub keywords: Vec<String>,
    pub severity: String,
    pub user_impact: String,
}

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

pub fn catalog_defs(catalog: &CatalogSnapshot) -> Vec<FeatureDef> {
    catalog
        .rules
        .iter()
        .map(|f| FeatureDef {
            id: f.id.clone(),
            name: f.name.clone(),
            category: f.category.clone(),
            description: f.description.clone(),
            user_facing: f.user_facing,
        })
        .collect()
}

pub fn match_features(
    catalog: &CatalogSnapshot,
    subject: &str,
    body: &str,
    files: &[String],
) -> Vec<FeatureHit> {
    let blob = format!("{subject}\n{body}").to_lowercase();
    let mut hits = Vec::new();

    for f in &catalog.rules {
        let mut reasons = Vec::new();
        let path_hits: Vec<&String> = files
            .iter()
            .filter(|path| rule_covers_path(f, path))
            .collect();
        if !path_hits.is_empty() {
            reasons.push(format!("触及 {} 个相关文件", path_hits.len()));
        }
        let kw: Vec<&String> = f
            .keywords
            .iter()
            .filter(|k| blob.contains(&k.to_lowercase()))
            .collect();
        if !kw.is_empty() {
            reasons.push(format!(
                "提交说明命中关键词: {}",
                kw.iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if reasons.is_empty() {
            continue;
        }
        hits.push(FeatureHit {
            id: f.id.clone(),
            name: f.name.clone(),
            category: f.category.clone(),
            severity: f.severity.clone(),
            why: reasons.join("；"),
            user_impact: f.user_impact.clone(),
        });
    }

    // Auto-discover modules not covered by curated path/name rules.
    let mut auto_ids = BTreeSet::new();
    for path in files {
        if catalog.path_covered_by_curated(path) {
            continue;
        }
        if let Some(hit) = auto_feature_for_path(path) {
            if auto_ids.insert(hit.id.clone()) {
                hits.push(hit);
            }
        }
    }

    hits
}

fn auto_feature_for_path(path: &str) -> Option<FeatureHit> {
    let norm = path.replace('\\', "/");
    let file_name = PathFile::new(&norm);
    let stem = file_name.stem?;
    let crate_name = if norm.contains("bony-build/") {
        "bony-build"
    } else if norm.contains("bony-monitor/") {
        "bony-monitor"
    } else if norm.starts_with("scripts/") {
        "scripts"
    } else if norm.starts_with("crates/codegen/") {
        let rest = norm.trim_start_matches("crates/codegen/");
        rest.split('/').next().unwrap_or("crate")
    } else {
        return None;
    };

    let id = format!("auto-{crate_name}-{stem}");
    let label = match crate_name {
        "bony-build" => format!("桌面·{stem}"),
        "bony-monitor" => format!("监控·{stem}"),
        "scripts" => format!("脚本·{stem}"),
        other => format!("{other}·{stem}"),
    };

    Some(FeatureHit {
        id,
        name: label.clone(),
        category: "自动发现".into(),
        severity: "medium".into(),
        why: format!("未覆盖路径：{norm}"),
        user_impact: format!("新模块「{label}」尚未写入功能目录，请确认是否归入既有 feature"),
    })
}

struct PathFile<'a> {
    stem: Option<&'a str>,
}

impl<'a> PathFile<'a> {
    fn new(path: &'a str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path);
        let stem = name.strip_suffix(".rs").or_else(|| {
            name.rsplit_once('.')
                .map(|(s, _)| s)
                .filter(|s| !s.is_empty())
        });
        Self { stem }
    }
}

pub fn match_dimensions(
    subject: &str,
    body: &str,
    files: &[String],
    features: &[FeatureHit],
) -> Vec<DimensionHit> {
    let mut out = Vec::new();
    let joined = files.join("\n").to_lowercase();
    let text = format!("{subject}\n{body}").to_lowercase();

    let has_ui = features
        .iter()
        .any(|f| f.category == "桌面体验" || f.id == "tui" || f.category == "自动发现");
    let has_cap = features.iter().any(|f| {
        matches!(
            f.id.as_str(),
            "tools"
                | "session-loop"
                | "multi-provider"
                | "model-picker"
                | "workspace-fs"
                | "projects"
                | "usage-analytics"
        ) || f.id.starts_with("auto-")
    });
    let has_sec = features
        .iter()
        .any(|f| f.id == "auth-login" || f.id == "permissions")
        || joined.contains("auth")
        || joined.contains("secret");

    if has_ui {
        out.push(dim("ux", "高", "界面或交互路径被改动，需目视回归主流程"));
    }
    if has_cap {
        out.push(dim(
            "capability",
            "高",
            "核心 Agent/模型/工具或桌面工作流能力可能变化",
        ));
    }
    if has_sec {
        out.push(dim(
            "security",
            "高",
            "认证或权限相关，需重点验证密钥与审批",
        ));
    }
    if features
        .iter()
        .any(|f| f.id == "session-loop" || f.id == "projects")
    {
        out.push(dim(
            "reliability",
            "高",
            "会话主链路或项目切换会重启 bridge，关注重连与取消",
        ));
    } else if !features.is_empty() {
        out.push(dim(
            "reliability",
            "中",
            "建议冒烟：启动 → 发一条消息 → 看工具是否正常",
        ));
    }
    if features
        .iter()
        .any(|f| f.id == "multi-provider" || f.id == "model-picker")
    {
        out.push(dim(
            "compat",
            "高",
            "不同供应商协议/鉴权可能不兼容，建议多模型抽测",
        ));
    }
    if text.contains("perf") || text.contains("性能") || text.contains("timeout") {
        out.push(dim("perf", "中", "提交提到性能/超时相关"));
    }
    if features
        .iter()
        .any(|f| f.id == "build-ci" || f.id == "monitor")
    {
        out.push(dim("dx", "中", "构建脚本或监控变更，影响开发与发布效率"));
    }
    if features.iter().any(|f| f.id == "docs-product") {
        out.push(dim("docs", "低", "文档更新，利于上手但不改运行时行为"));
    }
    if features.iter().any(|f| f.id.starts_with("auto-")) {
        out.push(dim(
            "dx",
            "中",
            "出现自动发现模块，建议补全 catalog/features.toml",
        ));
    }

    if out.is_empty() && !files.is_empty() {
        out.push(dim(
            "capability",
            "低",
            "未匹配到已知功能规则，请人工确认影响面",
        ));
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
    catalog: &CatalogSnapshot,
    subject: &str,
    body: &str,
    files: &[String],
) -> (Vec<FeatureHit>, Vec<DimensionHit>, Vec<String>) {
    let features = match_features(catalog, subject, body, files);
    let dimensions = match_dimensions(subject, body, files, &features);
    let mut checklist = Vec::new();
    for f in &features {
        checklist.push(format!("验证「{}」：{}", f.name, f.user_impact));
    }
    if features
        .iter()
        .any(|f| f.id == "chat" || f.id == "session-loop")
    {
        checklist.push("冒烟：启动桌面 → 发送一条消息 → 确认流式回复".into());
    }
    if features.iter().any(|f| f.id == "desktop-shell") {
        checklist.push("冒烟：顶栏菜单/前进后退/关闭按钮清晰；侧栏入口可点；左右栏可开关".into());
    }
    if features.iter().any(|f| f.id == "projects") {
        checklist.push("冒烟：打开项目选文件夹 → Agent 重连到新 cwd → 侧栏项目高亮".into());
    }
    if features.iter().any(|f| f.id == "usage-analytics") {
        checklist.push("冒烟：打开使用统计 → 统计图/模型/轮次三栏与 Y 轴标签可读".into());
    }
    if features.iter().any(|f| f.id == "task-history") {
        checklist.push("冒烟：新建任务清空时间线；点击历史任务只读回看".into());
    }
    if features
        .iter()
        .any(|f| f.id == "model-picker" || f.id == "multi-provider")
    {
        checklist.push("冒烟：切换至少 2 个模型并各发一条短消息".into());
    }
    if features.iter().any(|f| f.id == "auth-login") {
        checklist.push("冒烟：无凭证提示登录 / 有 Key 可直连".into());
    }
    if features
        .iter()
        .any(|f| f.id == "tools" || f.id == "permissions")
    {
        checklist.push("冒烟：触发一次读文件或列目录工具".into());
    }
    if features.iter().any(|f| f.id.starts_with("auto-")) {
        checklist.push(
            "目录：将自动发现模块并入 catalog/features.toml 对应功能，避免长期依赖 auto-*".into(),
        );
    }
    checklist.sort();
    checklist.dedup();
    (features, dimensions, checklist)
}

pub fn features_overview(catalog: &CatalogSnapshot, changes: &[ChangeEntry]) -> FeaturesOverview {
    let mut defs: Vec<FeatureDef> = catalog_defs(catalog);
    let mut seen: BTreeSet<String> = defs.iter().map(|d| d.id.clone()).collect();

    for c in changes {
        for h in &c.impact.features {
            if seen.insert(h.id.clone()) {
                defs.push(FeatureDef {
                    id: h.id.clone(),
                    name: h.name.clone(),
                    category: h.category.clone(),
                    description: h.user_impact.clone(),
                    user_facing: true,
                });
            }
        }
    }

    let mut activity = Vec::new();
    for f in &defs {
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
            feature: f.clone(),
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
        catalog: defs,
        activity,
        dimensions: DIMENSIONS.iter().map(|(_, l)| (*l).to_string()).collect(),
    }
}
