//! Change × capability impact matrix for working tree and recent commits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::catalog::CatalogSnapshot;
use crate::features::{FeatureDef, FeatureHit, catalog_defs};
use crate::git::{self, ChangeEntry};

#[derive(Debug, Clone, Serialize)]
pub struct MatrixChange {
    pub id: String,
    pub label: String,
    pub source: String,
    pub subject: String,
    pub additions: u32,
    pub deletions: u32,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactCell {
    pub change_id: String,
    pub capability_id: String,
    pub direction: String,
    pub magnitude: String,
    pub confidence: String,
    pub validation: String,
    pub why: String,
    pub user_effect: String,
    pub risks: Vec<String>,
    pub checklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactMatrix {
    pub changes: Vec<MatrixChange>,
    pub capabilities: Vec<FeatureDef>,
    pub cells: Vec<ImpactCell>,
    pub unverified_high: usize,
    pub unknown_direction: usize,
}

pub fn build(repo: &Path, limit: usize, catalog: &CatalogSnapshot) -> Result<ImpactMatrix> {
    let mut entries = git::working_changes(repo, catalog)?;
    entries.extend(git::list_changes(repo, limit, catalog)?);

    let mut changes = Vec::new();
    let mut cells = Vec::new();
    let mut touched = BTreeSet::new();
    for entry in &entries {
        let source = source(entry);
        changes.push(MatrixChange {
            id: entry.short_sha.clone(),
            label: label(entry),
            source: source.into(),
            subject: entry.subject.clone(),
            additions: entry.additions,
            deletions: entry.deletions,
            file_count: entry.files.len(),
        });
        for feature in &entry.impact.features {
            touched.insert(feature.id.clone());
            cells.push(cell(entry, feature));
        }
    }

    let mut defs: BTreeMap<String, FeatureDef> = catalog_defs(catalog)
        .into_iter()
        .map(|f| (f.id.clone(), f))
        .collect();
    for entry in &entries {
        for hit in &entry.impact.features {
            defs.entry(hit.id.clone()).or_insert_with(|| FeatureDef {
                id: hit.id.clone(),
                name: hit.name.clone(),
                category: hit.category.clone(),
                description: hit.user_impact.clone(),
                user_facing: true,
            });
        }
    }
    let mut capabilities: Vec<_> = touched
        .into_iter()
        .filter_map(|id| defs.remove(&id))
        .collect();
    capabilities.sort_by(|a, b| {
        b.user_facing
            .cmp(&a.user_facing)
            .then(a.category.cmp(&b.category))
            .then(a.name.cmp(&b.name))
    });

    let unverified_high = cells
        .iter()
        .filter(|c| c.magnitude == "high" && c.validation == "unverified")
        .count();
    let unknown_direction = cells.iter().filter(|c| c.direction == "unknown").count();
    Ok(ImpactMatrix {
        changes,
        capabilities,
        cells,
        unverified_high,
        unknown_direction,
    })
}

fn source(entry: &ChangeEntry) -> &'static str {
    match entry.short_sha.as_str() {
        "unstaged" => "unstaged",
        "staged" => "staged",
        _ => "commit",
    }
}

fn label(entry: &ChangeEntry) -> String {
    match source(entry) {
        "unstaged" => "未暂存".into(),
        "staged" => "已暂存".into(),
        _ => entry.short_sha.clone(),
    }
}

fn cell(entry: &ChangeEntry, feature: &FeatureHit) -> ImpactCell {
    let text = format!("{}\n{}", entry.subject, entry.body).to_lowercase();
    let direction = if contains_any(&text, &["regress", "break", "失败", "退化", "回滚"]) {
        "regress"
    } else if contains_any(
        &text,
        &["fix", "improve", "support", "新增", "修复", "增强", "支持"],
    ) {
        "improve"
    } else if contains_any(
        &text,
        &["refactor", "format", "rename", "重构", "格式", "重命名"],
    ) {
        "neutral"
    } else {
        "unknown"
    };
    let magnitude = match feature.severity.as_str() {
        "critical" | "high" => "high",
        "medium" => "medium",
        _ => "low",
    };
    let confidence = if feature.id.starts_with("auto-") {
        "low"
    } else if feature.why.contains("相关文件") && feature.why.contains("关键词") {
        "high"
    } else {
        "medium"
    };
    let validation = if text.lines().any(|line| {
        line.trim_start().starts_with("validate:") || line.trim_start().starts_with("验证:")
    }) {
        "declared"
    } else {
        "unverified"
    };
    ImpactCell {
        change_id: entry.short_sha.clone(),
        capability_id: feature.id.clone(),
        direction: direction.into(),
        magnitude: magnitude.into(),
        confidence: confidence.into(),
        validation: validation.into(),
        why: feature.why.clone(),
        user_effect: feature.user_impact.clone(),
        risks: entry.impact.risks.clone(),
        checklist: entry.impact.checklist.clone(),
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::FeatureHit;
    use crate::impact::ChangeImpact;

    fn entry(subject: &str, body: &str) -> ChangeEntry {
        ChangeEntry {
            sha: "a".repeat(40),
            short_sha: "aaaaaaa".into(),
            subject: subject.into(),
            body: body.into(),
            author: String::new(),
            email: String::new(),
            date: String::new(),
            additions: 1,
            deletions: 0,
            files: vec![],
            impact: ChangeImpact {
                areas: vec![],
                dimensions: vec![],
                improvements: vec![],
                tags: vec![],
                risks: vec!["risk".into()],
                checklist: vec!["test".into()],
                features: vec![],
            },
        }
    }

    #[test]
    fn direction_and_validation_are_evidence_based() {
        let hit = FeatureHit {
            id: "tasks".into(),
            name: "Tasks".into(),
            category: "Desktop".into(),
            severity: "high".into(),
            why: "触及 2 个相关文件".into(),
            user_impact: "resume works".into(),
        };
        let value = cell(&entry("Fix task resume", "Validate: cargo test"), &hit);
        assert_eq!(value.direction, "improve");
        assert_eq!(value.validation, "declared");
        assert_eq!(value.confidence, "medium");
    }
}
