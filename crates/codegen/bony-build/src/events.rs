//! Cross-thread messages between the egui UI and the ACP agent bridge.

use std::path::PathBuf;

/// Commands sent from the UI thread to the agent bridge.
#[derive(Debug)]
pub enum UiCommand {
    Prompt(String),
    Cancel,
    /// `true` = allow once, `false` = deny / cancel.
    PermissionResponse { allow: bool },
    /// Run `grok login` (browser) then reconnect the agent.
    Login,
    /// Switch the active session model via ACP `session/set_model`.
    SetModel { model_id: String },
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Events pushed from the agent bridge to the UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Status(String),
    /// Need browser login before chatting.
    NeedsLogin { message: String },
    Connected {
        session_id: String,
        cwd: PathBuf,
        current_model_id: String,
        current_model_name: String,
        models: Vec<ModelChoice>,
    },
    Disconnected,
    ModelChanged {
        model_id: String,
        name: String,
    },
    AssistantDelta(String),
    ToolStart { id: String, title: String },
    ToolUpdate { id: String, status: String, detail: String },
    PermissionRequest {
        tool_call_id: String,
        title: String,
        options: Vec<PermissionOptionView>,
    },
    TurnDone { stop_reason: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct PermissionOptionView {
    pub id: String,
    pub name: String,
    pub kind: String,
}
