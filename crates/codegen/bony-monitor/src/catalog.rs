//! Hot-reloadable feature catalog + workspace module discovery.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::features::FeatureRule;

#[derive(Debug, Clone, Deserialize)]
struct FeaturesFile {
    features: Vec<FeatureRuleToml>,
}

#[derive(Debug, Clone, Deserialize)]
struct FeatureRuleToml {
    id: String,
    name: String,
    category: String,
    description: String,
    #[serde(default = "default_true")]
    user_facing: bool,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default = "default_medium")]
    severity: String,
    #[serde(default)]
    user_impact: String,
}

fn default_true() -> bool {
    true
}

fn default_medium() -> String {
    "medium".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveredFile {
    pub path: String,
    pub crate_name: String,
    pub stem: String,
}

#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub rules: Vec<FeatureRule>,
    pub discovered: Vec<DiscoveredFile>,
    pub desktop_module_count: usize,
    pub loaded_at: SystemTime,
}

impl CatalogSnapshot {
    pub fn empty() -> Self {
        Self {
            rules: Vec::new(),
            discovered: Vec::new(),
            desktop_module_count: 0,
            loaded_at: SystemTime::now(),
        }
    }

    pub fn path_covered_by_curated(&self, path: &str) -> bool {
        self.rules.iter().any(|f| rule_covers_path(f, path))
    }
}

pub fn rule_covers_path(f: &FeatureRule, path: &str) -> bool {
    let path_l = path.to_ascii_lowercase();
    f.paths
        .iter()
        .any(|p| path.starts_with(p.as_str()) || path == p.as_str())
        || f.names.iter().any(|n| path_l.contains(&n.to_ascii_lowercase()))
}

pub struct CatalogCache {
    repo: PathBuf,
    catalog_dir: PathBuf,
    features_toml: PathBuf,
    inner: RwLock<CachedState>,
}

struct CachedState {
    snapshot: CatalogSnapshot,
    toml_mtime: Option<SystemTime>,
    scan_mtime: Option<SystemTime>,
}

impl CatalogCache {
    pub fn new(repo: PathBuf) -> Self {
        let catalog_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog");
        let features_toml = catalog_dir.join("features.toml");
        let cache = Self {
            repo,
            catalog_dir,
            features_toml,
            inner: RwLock::new(CachedState {
                snapshot: CatalogSnapshot::empty(),
                toml_mtime: None,
                scan_mtime: None,
            }),
        };
        cache.ensure_fresh();
        cache
    }

    pub fn snapshot(&self) -> CatalogSnapshot {
        self.ensure_fresh();
        self.inner
            .read()
            .expect("catalog lock")
            .snapshot
            .clone()
    }

    pub fn ensure_fresh(&self) {
        let toml_m = file_mtime(&self.features_toml);
        let scan_m = scan_tree_mtime(&self.repo);

        let needs = {
            let guard = self.inner.read().expect("catalog lock");
            guard.toml_mtime != toml_m || guard.scan_mtime != scan_m || guard.snapshot.rules.is_empty()
        };
        if !needs {
            return;
        }

        match load_snapshot(&self.repo, &self.features_toml, &self.catalog_dir) {
            Ok(snap) => {
                let mut guard = self.inner.write().expect("catalog lock");
                guard.snapshot = snap;
                guard.toml_mtime = toml_m;
                guard.scan_mtime = scan_m;
                tracing::info!("monitor catalog reloaded");
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to reload monitor catalog");
            }
        }
    }
}

fn load_snapshot(repo: &Path, features_toml: &Path, catalog_dir: &Path) -> Result<CatalogSnapshot> {
    let rules = load_features_toml(features_toml)?;
    let discovered = scan_modules(repo);
    let desktop_module_count = discovered
        .iter()
        .filter(|d| d.crate_name == "bony-build")
        .count();

    // Persist discovered.json for sync script / git visibility (best effort).
    let discovered_path = catalog_dir.join("discovered.json");
    if let Ok(text) = serde_json::to_string_pretty(&serde_json::json!({
        "generated_at": chrono_like_now(),
        "modules": discovered,
    })) {
        let _ = std::fs::create_dir_all(catalog_dir);
        let _ = std::fs::write(discovered_path, text);
    }

    Ok(CatalogSnapshot {
        rules,
        discovered,
        desktop_module_count,
        loaded_at: SystemTime::now(),
    })
}

fn load_features_toml(path: &Path) -> Result<Vec<FeatureRule>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let file: FeaturesFile = toml::from_str(&text).context("parse features.toml")?;
    Ok(file
        .features
        .into_iter()
        .map(|f| FeatureRule {
            id: f.id,
            name: f.name,
            category: f.category,
            description: f.description,
            user_facing: f.user_facing,
            paths: f.paths,
            names: f.names,
            keywords: f.keywords,
            severity: f.severity,
            user_impact: f.user_impact,
        })
        .collect())
}

fn scan_modules(repo: &Path) -> Vec<DiscoveredFile> {
    let mut out = Vec::new();
    let roots = [
        ("bony-build", repo.join("crates/codegen/bony-build/src")),
        ("bony-monitor", repo.join("crates/codegen/bony-monitor/src")),
    ];
    for (crate_name, dir) in roots {
        collect_rs(&dir, crate_name, repo, &mut out);
    }
    for script in ["scripts/run-desktop.ps1", "scripts/run-monitor.ps1"] {
        let p = repo.join(script);
        if p.is_file() {
            out.push(DiscoveredFile {
                path: script.replace('\\', "/"),
                crate_name: "scripts".into(),
                stem: Path::new(script)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| script.into()),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn collect_rs(dir: &Path, crate_name: &str, repo: &Path, out: &mut Vec<DiscoveredFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, crate_name, repo, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let rel = path
            .strip_prefix(repo)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(DiscoveredFile {
            path: rel,
            crate_name: crate_name.into(),
            stem,
        });
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn scan_tree_mtime(repo: &Path) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = None;
    for dir in [
        repo.join("crates/codegen/bony-build/src"),
        repo.join("crates/codegen/bony-monitor/src"),
        repo.join("scripts"),
    ] {
        bump_mtime_dir(&dir, &mut latest);
    }
    latest
}

fn bump_mtime_dir(dir: &Path, latest: &mut Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bump_mtime_dir(&path, latest);
            continue;
        }
        if let Ok(m) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            *latest = Some(match *latest {
                Some(cur) => cur.max(m),
                None => m,
            });
        }
    }
}

fn chrono_like_now() -> String {
    // Avoid extra chrono dep; ISO-ish local stamp is enough for the JSON file.
    format!("{:?}", SystemTime::now())
}
