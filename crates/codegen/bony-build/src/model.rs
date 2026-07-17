//! UI-facing session state (Codex-style timeline).

use std::path::PathBuf;

use crate::events::{AgentEvent, ModelChoice, PermissionOptionView};

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

#[derive(Debug, Default)]
pub struct AppModel {
    pub status: String,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub timeline: Vec<TimelineItem>,
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
}

impl AppModel {
    pub fn new() -> Self {
        Self {
            status: "Connecting…".into(),
            auto_scroll: true,
            login_message: "Sign in to chat with Bony Build.".into(),
            current_model_id: String::new(),
            current_model_name: "model".into(),
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
                self.busy = true;
                self.status = "Working…".into();
                match self.timeline.last_mut() {
                    Some(TimelineItem::Message(m)) if m.role == Role::Assistant => {
                        m.text.push_str(&delta);
                    }
                    _ => self.timeline.push(TimelineItem::Message(ChatMessage {
                        role: Role::Assistant,
                        text: delta,
                    })),
                }
            }
            AgentEvent::ToolStart { id, title } => {
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
            AgentEvent::TurnDone { stop_reason: _ } => {
                self.busy = false;
                self.status = "Ready".into();
            }
            AgentEvent::Error(err) => {
                self.busy = false;
                self.status = "Error".into();
                self.timeline.push(TimelineItem::Message(ChatMessage {
                    role: Role::System,
                    text: err,
                }));
            }
        }
    }

    pub fn push_user(&mut self, text: String) {
        self.timeline.push(TimelineItem::Message(ChatMessage {
            role: Role::User,
            text,
        }));
        self.busy = true;
        self.status = "Working…".into();
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
}
