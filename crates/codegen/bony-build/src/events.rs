//! Cross-thread messages between the egui UI and the ACP agent bridge.

use std::path::PathBuf;

use crate::usage::TokenUsage;

/// Commands sent from the UI thread to the agent bridge.
#[derive(Debug)]
pub enum UiCommand {
    Prompt {
        text: String,
        attachments: Vec<AttachmentPayload>,
    },
    /// Soft cancel of the in-flight turn (ACP `session/cancel`).
    Cancel,
    /// Kill the agent subprocess and reconnect — used when soft cancel cannot
    /// unblock a hung tool (e.g. missing `unity` hanging on Windows).
    ForceStop,
    /// Exact ACP permission option selected by the user; `None` cancels.
    PermissionResponse {
        option_id: Option<String>,
    },
    /// Run `grok login` (browser) then reconnect the agent.
    Login,
    /// Switch the active session model via ACP `session/set_model`.
    SetModel {
        model_id: String,
    },
    SetMode {
        mode_id: String,
    },
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct AttachmentPayload {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
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
    NeedsLogin {
        message: String,
    },
    Connected {
        session_id: String,
        cwd: PathBuf,
        current_model_id: String,
        current_model_name: String,
        models: Vec<ModelChoice>,
        current_mode_id: String,
        modes: Vec<ModeChoice>,
        restored: bool,
    },
    Disconnected,
    ModelChanged {
        model_id: String,
        name: String,
    },
    ModeChanged {
        mode_id: String,
    },
    AssistantDelta(String),
    ToolStart {
        id: String,
        title: String,
    },
    ToolUpdate {
        id: String,
        status: String,
        detail: String,
    },
    PermissionRequest {
        tool_call_id: String,
        title: String,
        options: Vec<PermissionOptionView>,
    },
    TurnDone {
        stop_reason: String,
        usage: TokenUsage,
    },
    /// Context window snapshot (when the agent emits UsageUpdate).
    ContextUsage {
        used: u64,
        size: u64,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ModeChoice {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct PermissionOptionView {
    pub id: String,
    pub name: String,
    pub kind: String,
}
