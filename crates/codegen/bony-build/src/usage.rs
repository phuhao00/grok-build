//! Token usage snapshots and per-turn conversation records.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: u64,
    pub cached_read_tokens: u64,
    /// Context window currently used (from UsageUpdate), if known.
    pub context_used: Option<u64>,
    /// Context window size, if known.
    pub context_size: Option<u64>,
}

impl TokenUsage {
    /// Add a per-turn bill onto session totals.
    pub fn add_turn(&mut self, turn: &TokenUsage) {
        self.total_tokens = self.total_tokens.saturating_add(turn.total_tokens);
        self.input_tokens = self.input_tokens.saturating_add(turn.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(turn.output_tokens);
        self.thought_tokens = self.thought_tokens.saturating_add(turn.thought_tokens);
        self.cached_read_tokens = self
            .cached_read_tokens
            .saturating_add(turn.cached_read_tokens);
        if turn.context_used.is_some() {
            self.context_used = turn.context_used;
        }
        if turn.context_size.is_some() {
            self.context_size = turn.context_size;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: String,
    pub session_id: String,
    pub model_id: String,
    pub model_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub stop_reason: String,
    pub user_text: String,
    pub assistant_text: String,
    pub tool_titles: Vec<String>,
    /// Cumulative usage reported at end of turn (from agent meta).
    pub usage_cumulative: TokenUsage,
    /// Delta vs previous cumulative snapshot (best effort).
    pub usage_delta: TokenUsage,
}

#[derive(Debug, Clone, Default)]
pub struct SessionUsageState {
    pub cumulative: TokenUsage,
    pub turns: Vec<TurnRecord>,
    /// User text of the in-flight turn (set on send).
    pub pending_user_text: String,
    pub pending_started_at: String,
}

impl SessionUsageState {
    pub fn begin_turn(&mut self, user_text: &str) {
        self.pending_user_text = user_text.to_string();
        self.pending_started_at = now_rfc3339();
    }

    pub fn finish_turn(
        &mut self,
        session_id: &str,
        model_id: &str,
        model_name: &str,
        stop_reason: &str,
        assistant_text: String,
        tool_titles: Vec<String>,
        reported: TokenUsage,
    ) -> TurnRecord {
        // Agent `_meta` sibling token fields / nested `usage` are per-prompt
        // (or last-call) bills — accumulate them for the session.
        let mut delta = reported;
        delta.context_used = delta.context_used.or(self.cumulative.context_used);
        delta.context_size = delta.context_size.or(self.cumulative.context_size);
        if delta.total_tokens == 0 && (delta.input_tokens > 0 || delta.output_tokens > 0) {
            delta.total_tokens = delta.input_tokens.saturating_add(delta.output_tokens);
        }
        self.cumulative.add_turn(&delta);

        let record = TurnRecord {
            id: format!(
                "{}-{}",
                &session_id[..session_id.len().min(8)],
                self.turns.len() + 1
            ),
            session_id: session_id.to_string(),
            model_id: model_id.to_string(),
            model_name: model_name.to_string(),
            started_at: if self.pending_started_at.is_empty() {
                now_rfc3339()
            } else {
                self.pending_started_at.clone()
            },
            finished_at: now_rfc3339(),
            stop_reason: stop_reason.to_string(),
            user_text: std::mem::take(&mut self.pending_user_text),
            assistant_text,
            tool_titles,
            usage_cumulative: self.cumulative.clone(),
            usage_delta: delta,
        };
        self.pending_started_at.clear();
        self.turns.push(record.clone());
        let _ = append_turn_record(&record);
        record
    }

    pub fn apply_context_window(&mut self, used: u64, size: u64) {
        self.cumulative.context_used = Some(used);
        self.cumulative.context_size = Some(size);
    }
}

pub fn usage_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".bony-build")
}

pub fn turns_log_path() -> PathBuf {
    usage_dir().join("turns.jsonl")
}

pub fn projects_path() -> PathBuf {
    usage_dir().join("projects.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectsFile {
    recent: Vec<PathBuf>,
}

pub fn load_recent_projects() -> Vec<PathBuf> {
    let path = projects_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_str::<ProjectsFile>(&text) else {
        return Vec::new();
    };
    file.recent
        .into_iter()
        .filter(|p| p.is_dir())
        .take(12)
        .collect()
}

pub fn save_recent_projects(projects: &[PathBuf]) {
    let dir = usage_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file = ProjectsFile {
        recent: projects.iter().take(12).cloned().collect(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(projects_path(), text);
    }
}

/// Move `path` to the front of the recent list and persist.
pub fn remember_project(projects: &mut Vec<PathBuf>, path: &Path) {
    let Ok(canonical) = path.canonicalize() else {
        // Still track the path even if canonicalize fails (e.g. not yet created).
        projects.retain(|p| p != path);
        projects.insert(0, path.to_path_buf());
        if projects.len() > 12 {
            projects.truncate(12);
        }
        save_recent_projects(projects);
        return;
    };
    projects.retain(|p| {
        p.canonicalize()
            .map(|c| c != canonical)
            .unwrap_or(true)
            && p != path
    });
    projects.insert(0, canonical);
    if projects.len() > 12 {
        projects.truncate(12);
    }
    save_recent_projects(projects);
}

pub fn append_turn_record(record: &TurnRecord) -> Result<(), String> {
    let dir = usage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = turns_log_path();
    let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

pub fn load_recent_turns(limit: usize) -> Vec<TurnRecord> {
    let path = turns_log_path();
    load_turns_from_path(&path, limit)
}

pub fn load_turns_from_path(path: &Path, limit: usize) -> Vec<TurnRecord> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<TurnRecord>(line) {
            out.push(r);
            if out.len() >= limit {
                break;
            }
        }
    }
    out.reverse();
    out
}

/// Parse per-prompt token bill from PromptResponse `_meta`.
///
/// Prefer nested `usage` (whole-prompt ledger). Fall back to sibling
/// `inputTokens` / `outputTokens` / `totalTokens` (last-call / turn totals).
pub fn parse_usage_from_meta(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> TokenUsage {
    let Some(meta) = meta else {
        return TokenUsage::default();
    };
    let nested = meta.get("usage");
    let mut u = TokenUsage {
        total_tokens: pick_u64(nested, meta, &["totalTokens", "total_tokens"]),
        input_tokens: pick_u64(nested, meta, &["inputTokens", "input_tokens"]),
        output_tokens: pick_u64(nested, meta, &["outputTokens", "output_tokens"]),
        thought_tokens: pick_u64(
            nested,
            meta,
            &["reasoningTokens", "thoughtTokens", "reasoning_tokens"],
        ),
        cached_read_tokens: pick_u64(
            nested,
            meta,
            &["cachedReadTokens", "cached_read_tokens"],
        ),
        context_used: None,
        context_size: None,
    };
    if u.total_tokens == 0 && (u.input_tokens > 0 || u.output_tokens > 0) {
        u.total_tokens = u.input_tokens.saturating_add(u.output_tokens);
    }
    u
}

fn pick_u64(
    nested: Option<&serde_json::Value>,
    meta: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> u64 {
    if let Some(usage) = nested {
        for key in keys {
            if usage.get(*key).is_some() {
                return json_u64(usage.get(*key));
            }
        }
    }
    for key in keys {
        if meta.get(*key).is_some() {
            return json_u64(meta.get(*key));
        }
    }
    0
}

fn json_u64(v: Option<&serde_json::Value>) -> u64 {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_i64().map(|i| i.max(0) as u64))
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Compact local-ish stamp without chrono dependency.
    format!("{secs}")
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n >= 1000 {
        format!("{:.2}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub session_id: String,
    pub title: String,
    pub turn_count: usize,
    pub total_tokens: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ModelUsageSummary {
    pub model_id: String,
    pub model_name: String,
    pub turn_count: usize,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Aggregate token usage by model (highest total first).
pub fn aggregate_model_usage(turns: &[TurnRecord]) -> Vec<ModelUsageSummary> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, ModelUsageSummary> =
        std::collections::HashMap::new();

    for turn in turns {
        let key = if turn.model_id.is_empty() {
            turn.model_name.clone()
        } else {
            turn.model_id.clone()
        };
        if key.is_empty() {
            continue;
        }
        let entry = map.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            ModelUsageSummary {
                model_id: turn.model_id.clone(),
                model_name: if turn.model_name.is_empty() {
                    turn.model_id.clone()
                } else {
                    turn.model_name.clone()
                },
                turn_count: 0,
                total_tokens: 0,
                input_tokens: 0,
                output_tokens: 0,
            }
        });
        if entry.model_name.is_empty() && !turn.model_name.is_empty() {
            entry.model_name = turn.model_name.clone();
        }
        entry.turn_count += 1;
        entry.total_tokens = entry
            .total_tokens
            .saturating_add(turn.usage_delta.total_tokens);
        entry.input_tokens = entry
            .input_tokens
            .saturating_add(turn.usage_delta.input_tokens);
        entry.output_tokens = entry
            .output_tokens
            .saturating_add(turn.usage_delta.output_tokens);
    }

    let mut models: Vec<ModelUsageSummary> = order
        .into_iter()
        .filter_map(|id| map.remove(&id))
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));
    models
}

/// Group turns by session_id (newest sessions first).
pub fn aggregate_tasks(turns: &[TurnRecord]) -> Vec<TaskSummary> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, TaskSummary> = std::collections::HashMap::new();

    for turn in turns {
        let entry = map.entry(turn.session_id.clone()).or_insert_with(|| {
            order.push(turn.session_id.clone());
            let title = truncate_title(&turn.user_text, 42);
            TaskSummary {
                session_id: turn.session_id.clone(),
                title: if title.is_empty() {
                    "未命名任务".into()
                } else {
                    title
                },
                turn_count: 0,
                total_tokens: 0,
                updated_at: turn.finished_at.clone(),
            }
        });
        entry.turn_count += 1;
        entry.total_tokens = entry
            .total_tokens
            .saturating_add(turn.usage_delta.total_tokens);
        entry.updated_at = turn.finished_at.clone();
        // Keep first user message as title (already set on insert).
    }

    // Newest updated sessions first.
    let mut tasks: Vec<TaskSummary> = order
        .into_iter()
        .filter_map(|id| map.remove(&id))
        .collect();
    tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    tasks
}

fn truncate_title(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        if ch == '\n' || ch == '\r' {
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
}
