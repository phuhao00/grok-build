//! Spawn `grok agent stdio` and bridge ACP to UI channels.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use agent_client_protocol::{self as acp, Agent as _};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use xai_acp_lib::LineBufferedRead;

use crate::events::{AgentEvent, ModelChoice, PermissionOptionView, UiCommand};

#[derive(Clone)]
pub struct BridgeConfig {
    pub grok_bin: PathBuf,
    pub cwd: PathBuf,
    pub always_approve: bool,
}

struct PendingPermission {
    respond: oneshot::Sender<bool>,
}

struct DesktopAcpClient {
    event_tx: std::sync::mpsc::Sender<AgentEvent>,
    pending: Arc<Mutex<Option<PendingPermission>>>,
    always_approve: bool,
    egui_ctx: egui::Context,
}

impl DesktopAcpClient {
    fn emit(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event);
        self.egui_ctx.request_repaint();
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Client for DesktopAcpClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        if self.always_approve {
            let outcome = pick_allow(&args.options)
                .map(|o| {
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(acp::RequestPermissionOutcome::Cancelled);
            return Ok(acp::RequestPermissionResponse::new(outcome));
        }

        let options: Vec<PermissionOptionView> = args
            .options
            .iter()
            .map(|o| PermissionOptionView {
                id: o.option_id.0.to_string(),
                name: o.name.clone(),
                kind: format!("{:?}", o.kind),
            })
            .collect();

        let title = args
            .tool_call
            .fields
            .title
            .clone()
            .unwrap_or_else(|| "Permission required".into());
        let tool_call_id = args.tool_call.tool_call_id.0.to_string();

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().unwrap();
            *guard = Some(PendingPermission { respond: tx });
        }

        self.emit(AgentEvent::PermissionRequest {
            tool_call_id,
            title,
            options,
        });

        let allow = rx.await.unwrap_or(false);
        let outcome = if allow {
            pick_allow(&args.options)
                .or(args.options.first())
                .map(|o| {
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(acp::RequestPermissionOutcome::Cancelled)
        } else {
            pick_reject(&args.options)
                .map(|o| {
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        o.option_id.clone(),
                    ))
                })
                .unwrap_or(acp::RequestPermissionOutcome::Cancelled)
        };

        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        match args.update {
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk { content, .. }) => {
                if let acp::ContentBlock::Text(text) = content
                    && !text.text.is_empty()
                {
                    self.emit(AgentEvent::AssistantDelta(text.text));
                }
            }
            acp::SessionUpdate::ToolCall(tc) => {
                let id = tc.tool_call_id.0.to_string();
                let title = tc.title.clone();
                self.emit(AgentEvent::ToolStart { id, title });
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                let status = update
                    .fields
                    .status
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_default();
                let mut detail = update.fields.title.clone().unwrap_or_default();
                if let Some(blocks) = update.fields.content.as_ref() {
                    let texts: Vec<String> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            acp::ToolCallContent::Content(c) => match &c.content {
                                acp::ContentBlock::Text(t) => Some(t.text.clone()),
                                _ => None,
                            },
                            acp::ToolCallContent::Diff(d) => Some(d.path.display().to_string()),
                            _ => None,
                        })
                        .collect();
                    if !texts.is_empty() {
                        if !detail.is_empty() {
                            detail.push('\n');
                        }
                        detail.push_str(&texts.join("\n"));
                    }
                }
                self.emit(AgentEvent::ToolUpdate { id, status, detail });
            }
            _ => {}
        }
        Ok(())
    }
}

fn pick_allow(options: &[acp::PermissionOption]) -> Option<&acp::PermissionOption> {
    options
        .iter()
        .find(|o| o.kind == acp::PermissionOptionKind::AllowOnce)
        .or_else(|| {
            options
                .iter()
                .find(|o| o.kind == acp::PermissionOptionKind::AllowAlways)
        })
}

fn pick_reject(options: &[acp::PermissionOption]) -> Option<&acp::PermissionOption> {
    options
        .iter()
        .find(|o| o.kind == acp::PermissionOptionKind::RejectOnce)
        .or_else(|| {
            options
                .iter()
                .find(|o| o.kind == acp::PermissionOptionKind::RejectAlways)
        })
}

/// Start the agent bridge on a background thread. Returns UI command sender.
pub fn spawn_bridge(
    config: BridgeConfig,
    egui_ctx: egui::Context,
    event_tx: std::sync::mpsc::Sender<AgentEvent>,
) -> mpsc::UnboundedSender<UiCommand> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();
    thread::Builder::new()
        .name("grok-agent-bridge".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async move {
                if let Err(err) = run_bridge_loop(config, egui_ctx, event_tx, cmd_rx).await {
                    tracing::error!(error = %err, "agent bridge failed");
                }
            }));
        })
        .expect("spawn agent bridge thread");
    cmd_tx
}

async fn run_bridge_loop(
    config: BridgeConfig,
    egui_ctx: egui::Context,
    event_tx: std::sync::mpsc::Sender<AgentEvent>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
) -> anyhow::Result<()> {
    loop {
        match run_session(&config, &egui_ctx, &event_tx, &mut cmd_rx).await {
            SessionEnd::Shutdown => break,
            SessionEnd::Reconnect => {
                let _ = event_tx.send(AgentEvent::Disconnected);
                egui_ctx.request_repaint();
                continue;
            }
            SessionEnd::Fatal(err) => {
                let _ = event_tx.send(AgentEvent::Error(err));
                egui_ctx.request_repaint();
                // Wait for Login / Shutdown so UI can recover.
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        UiCommand::Shutdown => return Ok(()),
                        UiCommand::Login => {
                            run_grok_login(&config, &event_tx, &egui_ctx).await;
                            break;
                        }
                        UiCommand::Prompt(_) => {
                            let _ = event_tx.send(AgentEvent::NeedsLogin {
                                message: "Please sign in before chatting.".into(),
                            });
                            egui_ctx.request_repaint();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

enum SessionEnd {
    Shutdown,
    Reconnect,
    Fatal(String),
}

async fn run_session(
    config: &BridgeConfig,
    egui_ctx: &egui::Context,
    event_tx: &std::sync::mpsc::Sender<AgentEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<UiCommand>,
) -> SessionEnd {
    let emit = |e: AgentEvent| {
        let _ = event_tx.send(e);
        egui_ctx.request_repaint();
    };

    emit(AgentEvent::Status(format!(
        "Starting agent ({})…",
        config.grok_bin.display()
    )));

    let mut child = match tokio::process::Command::new(&config.grok_bin)
        .args(["agent", "stdio"])
        .current_dir(&config.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return SessionEnd::Fatal(format!(
                "failed to spawn `{} agent stdio`: {e}",
                config.grok_bin.display()
            ));
        }
    };

    let outgoing = match child.stdin.take() {
        Some(s) => s.compat_write(),
        None => return SessionEnd::Fatal("missing stdin".into()),
    };
    let incoming = match child.stdout.take() {
        Some(s) => s.compat(),
        None => return SessionEnd::Fatal("missing stdout".into()),
    };

    let stderr_buf = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let stderr_buf = stderr_buf.clone();
        tokio::task::spawn_local(async move {
            use tokio::io::AsyncReadExt;
            let mut stderr = stderr;
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let line = String::from_utf8_lossy(&buf[..n]);
                        if let Ok(mut g) = stderr_buf.lock() {
                            g.push_str(&line);
                            if g.len() > 8000 {
                                let len = g.len();
                                let keep = g.split_off(len - 4000);
                                *g = keep;
                            }
                        }
                        tracing::info!(target: "grok-agent", "{line}");
                    }
                }
            }
        });
    }

    let pending = Arc::new(Mutex::new(None));
    let client = DesktopAcpClient {
        event_tx: event_tx.clone(),
        pending: pending.clone(),
        always_approve: config.always_approve,
        egui_ctx: egui_ctx.clone(),
    };

    let incoming = LineBufferedRead::spawn_local(incoming);
    let (conn, handle_io) = acp::ClientSideConnection::new(client, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(handle_io);

    emit(AgentEvent::Status("Initializing…".into()));
    let init = match conn
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_capabilities(
                    acp::ClientCapabilities::new()
                        .fs(acp::FileSystemCapabilities::new())
                        .terminal(false),
                )
                .client_info(
                    acp::Implementation::new("xai-grok-desktop", env!("CARGO_PKG_VERSION"))
                        .title("Grok Desktop"),
                )
                .meta(
                    serde_json::json!({
                        "clientType": "desktop",
                        "clientVersion": env!("CARGO_PKG_VERSION"),
                    })
                    .as_object()
                    .cloned(),
                ),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let tail = stderr_buf.lock().ok().map(|g| g.clone()).unwrap_or_default();
            let _ = child.kill().await;
            return SessionEnd::Fatal(format!("initialize failed: {e}\n{tail}"));
        }
    };

    let method_ids: Vec<String> = init
        .auth_methods
        .iter()
        .map(|m| m.id().0.to_string())
        .collect();
    tracing::info!(?method_ids, "auth methods");

    let default_id = init
        .meta
        .as_ref()
        .and_then(|m| m.get("defaultAuthMethodId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let has_local_creds = std::path::Path::new(&std::env::var_os("USERPROFILE").unwrap_or_default())
        .join(".grok")
        .join("auth.json")
        .is_file()
        || std::env::var_os("XAI_API_KEY").is_some()
        || std::env::var_os("GROK_CODE_XAI_API_KEY").is_some();

    if !has_local_creds
        && !method_ids.iter().any(|id| id == "cached_token" || id == "xai.api_key")
    {
        let _ = child.kill().await;
        emit(AgentEvent::NeedsLogin {
            message: "Sign in with Grok, or configure any LLM in ~/.grok/config.toml ([model.*] + api_key/env_key) and restart.".into(),
        });
        // Wait for Login / Shutdown before reconnecting.
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                UiCommand::Shutdown => return SessionEnd::Shutdown,
                UiCommand::Login => {
                    run_grok_login(config, event_tx, egui_ctx).await;
                    return SessionEnd::Reconnect;
                }
                UiCommand::Prompt(_) => {
                    emit(AgentEvent::NeedsLogin {
                        message: "Please sign in first, then send a message.".into(),
                    });
                }
                _ => {}
            }
        }
        return SessionEnd::Shutdown;
    }

    if let Some(method) = select_auth_method(&init.auth_methods, default_id.as_deref()) {
        let id = method.id().0.to_string();
        emit(AgentEvent::Status(format!("Signing in ({id})…")));
        let mut req = acp::AuthenticateRequest::new(method.id().clone());
        // Interactive browser methods must NOT be headless.
        if id != "grok.com" && !id.contains("oidc") {
            req = req.meta(serde_json::json!({ "headless": true }).as_object().cloned());
        }
        if let Err(e) = conn.authenticate(req).await {
            let _ = child.kill().await;
            emit(AgentEvent::NeedsLogin {
                message: format!("Sign-in failed: {e}. Click Sign in to open the browser."),
            });
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    UiCommand::Shutdown => return SessionEnd::Shutdown,
                    UiCommand::Login => {
                        run_grok_login(config, event_tx, egui_ctx).await;
                        return SessionEnd::Reconnect;
                    }
                    UiCommand::Prompt(_) => {
                        emit(AgentEvent::NeedsLogin {
                            message: "Please sign in first.".into(),
                        });
                    }
                    _ => {}
                }
            }
            return SessionEnd::Shutdown;
        }
    }

    emit(AgentEvent::Status("Opening session…".into()));
    let session = match conn
        .new_session(acp::NewSessionRequest::new(config.cwd.clone()).mcp_servers(vec![]))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill().await;
            return SessionEnd::Fatal(format!(
                "session/new failed: {e}. Try Sign in again."
            ));
        }
    };

    let session_id = session.session_id.clone();
    let (current_model_id, current_model_name, mut models) = extract_models(&session);
    emit(AgentEvent::Connected {
        session_id: session_id.0.to_string(),
        cwd: config.cwd.clone(),
        current_model_id,
        current_model_name,
        models: models.clone(),
    });

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            UiCommand::Shutdown => {
                let _ = child.kill().await;
                return SessionEnd::Shutdown;
            }
            UiCommand::Login => {
                let _ = child.kill().await;
                run_grok_login(config, event_tx, egui_ctx).await;
                return SessionEnd::Reconnect;
            }
            UiCommand::SetModel { model_id } => {
                emit(AgentEvent::Status(format!("切换模型 {model_id}…")));
                match conn
                    .set_session_model(acp::SetSessionModelRequest::new(
                        session_id.clone(),
                        acp::ModelId::new(model_id.clone()),
                    ))
                    .await
                {
                    Ok(_) => {
                        if let Err(e) = crate::config_io::set_default_model(&model_id) {
                            tracing::warn!(error = %e, "failed to persist default model");
                        }
                        let name = models
                            .iter()
                            .find(|m| m.id == model_id)
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| model_id.clone());
                        // Keep local catalog in sync for subsequent switches.
                        if !models.iter().any(|m| m.id == model_id) {
                            models.push(ModelChoice {
                                id: model_id.clone(),
                                name: name.clone(),
                                description: String::new(),
                            });
                        }
                        emit(AgentEvent::ModelChanged { model_id, name });
                    }
                    Err(e) => {
                        emit(AgentEvent::Error(format!("切换模型失败: {e}")));
                    }
                }
            }
            UiCommand::Cancel => {
                let _ = conn
                    .cancel(acp::CancelNotification::new(session_id.clone()))
                    .await;
            }
            UiCommand::PermissionResponse { allow } => {
                if let Some(pending) = pending.lock().unwrap().take() {
                    let _ = pending.respond.send(allow);
                }
            }
            UiCommand::Prompt(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                emit(AgentEvent::Status("Thinking…".into()));
                match conn
                    .prompt(acp::PromptRequest::new(
                        session_id.clone(),
                        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
                    ))
                    .await
                {
                    Ok(resp) => {
                        emit(AgentEvent::TurnDone {
                            stop_reason: format!("{:?}", resp.stop_reason),
                        });
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.to_lowercase().contains("auth")
                            || msg.to_lowercase().contains("unauthor")
                        {
                            emit(AgentEvent::NeedsLogin {
                                message: format!("Auth expired: {msg}. Please sign in again."),
                            });
                        } else {
                            emit(AgentEvent::Error(format!("prompt failed: {msg}")));
                        }
                    }
                }
            }
        }
    }

    let _ = child.kill().await;
    SessionEnd::Shutdown
}

async fn run_grok_login(
    config: &BridgeConfig,
    event_tx: &std::sync::mpsc::Sender<AgentEvent>,
    egui_ctx: &egui::Context,
) {
    let _ = event_tx.send(AgentEvent::Status(
        "Opening browser for sign-in… complete login, then return here.".into(),
    ));
    egui_ctx.request_repaint();

    let grok_bin = config.grok_bin.clone();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&grok_bin)
            .arg("login")
            .status()
    })
    .await;

    match result {
        Ok(Ok(status)) if status.success() => {
            let _ = event_tx.send(AgentEvent::Status("Signed in. Reconnecting…".into()));
        }
        Ok(Ok(status)) => {
            let _ = event_tx.send(AgentEvent::Error(format!(
                "grok login exited with {status}. Try again."
            )));
        }
        Ok(Err(e)) => {
            let _ = event_tx.send(AgentEvent::Error(format!("failed to run grok login: {e}")));
        }
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Error(format!("login task failed: {e}")));
        }
    }
    egui_ctx.request_repaint();
}

fn select_auth_method<'a>(
    methods: &'a [acp::AuthMethod],
    default_id: Option<&str>,
) -> Option<&'a acp::AuthMethod> {
    if methods.is_empty() {
        return None;
    }
    if let Some(id) = default_id
        && let Some(m) = methods.iter().find(|m| m.id().0.as_ref() == id)
    {
        return Some(m);
    }
    methods
        .iter()
        .find(|m| m.id().0.as_ref() == "cached_token")
        .or_else(|| methods.iter().find(|m| m.id().0.as_ref() == "xai.api_key"))
        .or_else(|| methods.iter().find(|m| m.id().0.as_ref() == "grok.com"))
        .or_else(|| methods.first())
}

fn extract_models(session: &acp::NewSessionResponse) -> (String, String, Vec<ModelChoice>) {
    if let Some(state) = session.models.as_ref() {
        let models: Vec<ModelChoice> = state
            .available_models
            .iter()
            .map(|m| ModelChoice {
                id: m.model_id.0.to_string(),
                name: m.name.clone(),
                description: m.description.clone().unwrap_or_default(),
            })
            .collect();
        let current_id = state.current_model_id.0.to_string();
        let current_name = models
            .iter()
            .find(|m| m.id == current_id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| current_id.clone());
        return (current_id, current_name, models);
    }
    ("".into(), "未设置".into(), Vec::new())
}

pub fn resolve_grok_bin(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    which_grok().unwrap_or_else(|| PathBuf::from("grok"))
}

fn which_grok() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    // Common npm global bin locations (often missing from GUI-app PATH on Windows).
    if let Some(roaming) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(roaming).join("npm"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("npm"));
    }
    // Prefer .cmd on Windows so we don't pick up the extensionless npm shim.
    for dir in dirs {
        for name in ["grok.cmd", "grok.exe", "grok.bat", "grok"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
