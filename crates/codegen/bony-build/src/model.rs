//! UI-facing session state (Codex-style timeline).

use std::path::PathBuf;

use crate::events::{AgentEvent, ModelChoice, PermissionOptionView};
use crate::usage::{
    aggregate_tasks, load_recent_turns, SessionUsageState, TaskSummary, TokenUsage, TurnRecord,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
    /// Per-turn token bill (set on assistant messages when a turn completes).
    pub turn_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
pub struct ToolCard {
    pub id: String,
    pub title: String,
    pub status: String,
    pub detail: String,
    pub open: bool,
}

#[derive(Debug, Clone)]
pub enum TimelineItem {
    Message(ChatMessage),
    Tool(ToolCard),
}

#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<PermissionOptionView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageTab {
    #[default]
    Models,
    Turns,
}

#[derive(Debug, Default)]
pub struct AppModel {
    pub status: String,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub timeline: Vec<TimelineItem>,
    /// Snapshot of live timeline while viewing a historical task.
    pub live_timeline: Vec<TimelineItem>,
    pub pending_permission: Option<PendingPermission>,
    pub busy: bool,
    pub draft: String,
    pub auto_scroll: bool,
    pub connected: bool,
    pub needs_login: bool,
    pub login_message: String,
    pub current_model_id: String,
    pub current_model_name: String,
    pub available_models: Vec<ModelChoice>,
    pub show_model_picker: bool,
    pub show_user_menu: bool,
    pub show_usage_detail: bool,
    pub usage_tab: UsageTab,
    /// `None` = live current session; `Some` = read-only history view.
    pub viewing_session_id: Option<String>,
    pub task_title: String,
    pub display_name: String,
    pub usage: SessionUsageState,
    /// Recent turns loaded from disk (includes prior sessions).
    pub history_turns: Vec<TurnRecord>,
    /// Expanded turn ids in the usage detail panel.
    pub history_expanded: Vec<String>,
}

impl AppModel {
    pub fn new() -> Self {
        Self {
            status: "Connecting…".into(),
            auto_scroll: true,
            login_message: "Sign in to chat with Bony Build.".into(),
            current_model_id: String::new(),
            current_model_name: "model".into(),
            task_title: "新任务".into(),
            display_name: default_display_name(),
            history_turns: load_recent_turns(80),
            ..Default::default()
        }
    }

    pub fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Status(s) => self.status = s,
            AgentEvent::NeedsLogin { message } => {
                self.needs_login = true;
                self.connected = false;
                self.busy = false;
                self.login_message = message;
                self.status = "Sign in required".into();
            }
            AgentEvent::Disconnected => {
                self.connected = false;
                self.session_id = None;
                self.status = "Reconnecting…".into();
            }
            AgentEvent::Connected {
                session_id,
                cwd,
                current_model_id,
                current_model_name,
                models,
            } => {
                self.session_id = Some(session_id);
                self.cwd = Some(cwd);
                self.connected = true;
                self.needs_login = false;
                self.current_model_id = current_model_id;
                self.current_model_name = current_model_name;
                self.available_models = models;
                self.status = "Ready".into();
            }
            AgentEvent::ModelChanged { model_id, name } => {
                self.current_model_id = model_id;
                self.current_model_name = name;
                self.show_model_picker = false;
                self.status = "Ready".into();
            }
            AgentEvent::AssistantDelta(delta) => {
                self.ensure_live_view();
                self.busy = true;
                self.status = "Working…".into();
                match self.timeline.last_mut() {
                    Some(TimelineItem::Message(m)) if m.role == Role::Assistant => {
                        m.text.push_str(&delta);
                    }
                    _ => self.timeline.push(TimelineItem::Message(ChatMessage {
                        role: Role::Assistant,
                        text: delta,
                        turn_usage: None,
                    })),
                }
            }
            AgentEvent::ToolStart { id, title } => {
                self.ensure_live_view();
                self.busy = true;
                self.status = "Running tools…".into();
                if let Some(card) = self.find_tool_mut(&id) {
                    card.title = title;
                    card.status = "InProgress".into();
                } else {
                    self.timeline.push(TimelineItem::Tool(ToolCard {
                        id,
                        title,
                        status: "InProgress".into(),
                        detail: String::new(),
                        open: false,
                    }));
                }
            }
            AgentEvent::ToolUpdate {
                id,
                status,
                detail,
            } => {
                self.ensure_live_view();
                if let Some(card) = self.find_tool_mut(&id) {
                    if !status.is_empty() {
                        card.status = status;
                    }
                    if !detail.is_empty() {
                        if !card.detail.is_empty() {
                            card.detail.push('\n');
                        }
                        card.detail.push_str(&detail);
                    }
                } else {
                    self.timeline.push(TimelineItem::Tool(ToolCard {
                        id,
                        title: "Tool".into(),
                        status,
                        detail,
                        open: false,
                    }));
                }
            }
            AgentEvent::PermissionRequest {
                tool_call_id,
                title,
                options,
            } => {
                self.pending_permission = Some(PendingPermission {
                    tool_call_id,
                    title,
                    options,
                });
                self.status = "Needs approval".into();
            }
            AgentEvent::ContextUsage { used, size } => {
                self.usage.apply_context_window(used, size);
            }
            AgentEvent::TurnDone { stop_reason, usage } => {
                self.ensure_live_view();
                self.busy = false;
                self.status = "Ready".into();
                let session_id = self
                    .session_id
                    .clone()
                    .unwrap_or_else(|| "local".into());
                let assistant = self.last_assistant_text();
                let tools = self.tools_since_last_user();
                let record = self.usage.finish_turn(
                    &session_id,
                    &self.current_model_id,
                    &self.current_model_name,
                    &stop_reason,
                    assistant,
                    tools,
                    usage,
                );
                if let Some(TimelineItem::Message(m)) = self.timeline.iter_mut().rev().find(|i| {
                    matches!(i, TimelineItem::Message(m) if m.role == Role::Assistant)
                }) {
                    m.turn_usage = Some(record.usage_delta.clone());
                }
                if self.task_title == "新任务" && !record.user_text.is_empty() {
                    self.task_title = truncate_chars(&record.user_text, 42);
                }
                self.history_turns.push(record);
                if self.history_turns.len() > 200 {
                    let drop_n = self.history_turns.len() - 200;
                    self.history_turns.drain(0..drop_n);
                }
            }
            AgentEvent::Error(err) => {
                self.ensure_live_view();
                self.busy = false;
                self.status = "Error".into();
                self.timeline.push(TimelineItem::Message(ChatMessage {
                    role: Role::System,
                    text: err,
                    turn_usage: None,
                }));
            }
        }
    }

    pub fn push_user(&mut self, text: String) {
        self.ensure_live_view();
        if self.task_title == "新任务" {
            self.task_title = truncate_chars(&text, 42);
        }
        self.usage.begin_turn(&text);
        self.timeline.push(TimelineItem::Message(ChatMessage {
            role: Role::User,
            text,
            turn_usage: None,
        }));
        self.busy = true;
        self.status = "Working…".into();
    }

    /// Clear local chat and start a fresh task UI (same ACP session).
    pub fn new_task(&mut self) {
        self.viewing_session_id = None;
        self.live_timeline.clear();
        self.timeline.clear();
        self.usage.turns.clear();
        self.usage.pending_user_text.clear();
        self.usage.pending_started_at.clear();
        // Keep cumulative session token totals across "new task" clears.
        self.task_title = "新任务".into();
        self.draft.clear();
        self.pending_permission = None;
        self.auto_scroll = true;
        self.show_user_menu = false;
        if self.connected && !self.needs_login {
            self.status = "Ready".into();
        }
    }

    /// Read-only replay of a historical session's turns.
    pub fn load_task_view(&mut self, session_id: &str) {
        if self.viewing_session_id.is_none() {
            self.live_timeline = self.timeline.clone();
        }
        self.viewing_session_id = Some(session_id.to_string());
        let turns: Vec<&TurnRecord> = self
            .history_turns
            .iter()
            .filter(|t| t.session_id == session_id)
            .collect();
        self.task_title = turns
            .first()
            .map(|t| truncate_chars(&t.user_text, 42))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "历史任务".into());
        let mut timeline = Vec::new();
        for turn in turns {
            if !turn.user_text.is_empty() {
                timeline.push(TimelineItem::Message(ChatMessage {
                    role: Role::User,
                    text: turn.user_text.clone(),
                    turn_usage: None,
                }));
            }
            for tool in &turn.tool_titles {
                timeline.push(TimelineItem::Tool(ToolCard {
                    id: format!("{}-{}", turn.id, tool),
                    title: tool.clone(),
                    status: "Completed".into(),
                    detail: String::new(),
                    open: false,
                }));
            }
            if !turn.assistant_text.is_empty() {
                timeline.push(TimelineItem::Message(ChatMessage {
                    role: Role::Assistant,
                    text: turn.assistant_text.clone(),
                    turn_usage: Some(turn.usage_delta.clone()),
                }));
            }
        }
        self.timeline = timeline;
        self.auto_scroll = true;
        self.show_user_menu = false;
    }

    pub fn return_to_live(&mut self) {
        if self.viewing_session_id.take().is_some() {
            self.timeline = std::mem::take(&mut self.live_timeline);
            self.task_title = self
                .timeline
                .iter()
                .find_map(|i| match i {
                    TimelineItem::Message(m) if m.role == Role::User => {
                        Some(truncate_chars(&m.text, 42))
                    }
                    _ => None,
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "新任务".into());
        }
    }

    fn ensure_live_view(&mut self) {
        if self.viewing_session_id.is_some() {
            self.return_to_live();
        }
    }

    pub fn tasks(&self) -> Vec<TaskSummary> {
        aggregate_tasks(&self.history_turns)
    }

    pub fn is_viewing_history(&self) -> bool {
        self.viewing_session_id.is_some()
    }

    pub fn initials(&self) -> String {
        let name = self.display_name.trim();
        let mut chars = name.chars().filter(|c| !c.is_whitespace());
        let a = chars.next().unwrap_or('B');
        let b = chars.next().unwrap_or('B');
        format!("{a}{b}").to_uppercase()
    }

    fn last_assistant_text(&self) -> String {
        for item in self.timeline.iter().rev() {
            if let TimelineItem::Message(m) = item
                && m.role == Role::Assistant
            {
                return m.text.clone();
            }
        }
        String::new()
    }

    fn tools_since_last_user(&self) -> Vec<String> {
        let mut titles = Vec::new();
        for item in self.timeline.iter().rev() {
            match item {
                TimelineItem::Message(m) if m.role == Role::User => break,
                TimelineItem::Tool(t) => titles.push(t.title.clone()),
                _ => {}
            }
        }
        titles.reverse();
        titles
    }

    fn find_tool_mut(&mut self, id: &str) -> Option<&mut ToolCard> {
        for item in self.timeline.iter_mut().rev() {
            if let TimelineItem::Tool(card) = item
                && card.id == id
            {
                return Some(card);
            }
        }
        None
    }

    pub fn is_empty_chat(&self) -> bool {
        self.timeline
            .iter()
            .all(|i| matches!(i, TimelineItem::Message(m) if m.role == Role::System))
            || self.timeline.is_empty()
    }

    pub fn toggle_history_expanded(&mut self, id: &str) {
        if let Some(pos) = self.history_expanded.iter().position(|x| x == id) {
            self.history_expanded.remove(pos);
        } else {
            self.history_expanded.push(id.to_string());
        }
    }

    pub fn is_history_expanded(&self, id: &str) -> bool {
        self.history_expanded.iter().any(|x| x == id)
    }
}

fn default_display_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "bony".into())
}

fn truncate_chars(s: &str, max: usize) -> String {
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
