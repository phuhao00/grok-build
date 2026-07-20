//! Spawn `grok agent stdio` and bridge ACP to UI channels.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use agent_client_protocol::{self as acp, Agent as _};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use xai_acp_lib::LineBufferedRead;

use crate::events::{AgentEvent, ModeChoice, ModelChoice, PermissionOptionView, UiCommand};
use crate::usage::parse_usage_from_meta;

#[derive(Clone)]
pub struct BridgeConfig {
    pub grok_bin: PathBuf,
    pub cwd: PathBuf,
    pub always_approve: bool,
    pub resume_session_id: Option<String>,
}

struct PendingPermission {
    respond: oneshot::Sender<Option<String>>,
}

struct DesktopAcpClient {
    event_tx: std::sync::mpsc::Sender<AgentEvent>,
    pending: Arc<Mutex<Option<PendingPermission>>>,
    always_approve: bool,
    egui_ctx: egui::Context,
    terminals: Arc<Mutex<HashMap<String, LocalTerminal>>>,
}

struct LocalTerminal {
    child: std::process::Child,
    output: Arc<Mutex<Vec<u8>>>,
    output_byte_limit: usize,
    exit_code: Option<u32>,
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

        let selected = rx.await.unwrap_or(None);
        let outcome = selected
            .and_then(|id| {
                args.options
                    .iter()
                    .find(|o| o.option_id.0.as_ref() == id)
                    .map(|o| o.option_id.clone())
            })
            .map(|id| {
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(id))
            })
            .unwrap_or(acp::RequestPermissionOutcome::Cancelled);

        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn create_terminal(
        &self,
        args: acp::CreateTerminalRequest,
    ) -> acp::Result<acp::CreateTerminalResponse> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = std::process::Command::new(&args.command);
        command
            .args(&args.args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(cwd) = args.cwd {
            command.current_dir(cwd);
        }
        for item in args.env {
            command.env(item.name, item.value);
        }
        let mut child = command
            .spawn()
            .map_err(|e| acp::Error::internal_error().data(e.to_string()))?;
        let output = Arc::new(Mutex::new(Vec::new()));
        for mut reader in [
            child
                .stdout
                .take()
                .map(|v| Box::new(v) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|v| Box::new(v) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let target = output.clone();
            std::thread::spawn(move || {
                let mut buf = [0_u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut out) = target.lock() {
                                out.extend_from_slice(&buf[..n]);
                            }
                        }
                    }
                }
            });
        }
        self.terminals.lock().unwrap().insert(
            id.clone(),
            LocalTerminal {
                child,
                output,
                output_byte_limit: args.output_byte_limit.unwrap_or(1024 * 1024) as usize,
                exit_code: None,
            },
        );
        Ok(acp::CreateTerminalResponse::new(acp::TerminalId::new(id)))
    }

    async fn terminal_output(
        &self,
        args: acp::TerminalOutputRequest,
    ) -> acp::Result<acp::TerminalOutputResponse> {
        let mut terminals = self.terminals.lock().unwrap();
        let terminal = terminals
            .get_mut(args.terminal_id.0.as_ref())
            .ok_or_else(|| acp::Error::invalid_params().data("unknown terminal"))?;
        update_exit(terminal)?;
        let bytes = terminal.output.lock().unwrap();
        let truncated = bytes.len() > terminal.output_byte_limit;
        let start = bytes.len().saturating_sub(terminal.output_byte_limit);
        let output = String::from_utf8_lossy(&bytes[start..]).into_owned();
        let mut response = acp::TerminalOutputResponse::new(output, truncated);
        if let Some(code) = terminal.exit_code {
            response = response.exit_status(acp::TerminalExitStatus::new().exit_code(code));
        }
        Ok(response)
    }

    async fn wait_for_terminal_exit(
        &self,
        args: acp::WaitForTerminalExitRequest,
    ) -> acp::Result<acp::WaitForTerminalExitResponse> {
        loop {
            if let Some(code) = {
                let mut terminals = self.terminals.lock().unwrap();
                let terminal = terminals
                    .get_mut(args.terminal_id.0.as_ref())
                    .ok_or_else(|| acp::Error::invalid_params().data("unknown terminal"))?;
                update_exit(terminal)?;
                terminal.exit_code
            } {
                return Ok(acp::WaitForTerminalExitResponse::new(
                    acp::TerminalExitStatus::new().exit_code(code),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
    }

    async fn kill_terminal(
        &self,
        args: acp::KillTerminalRequest,
    ) -> acp::Result<acp::KillTerminalResponse> {
        if let Some(terminal) = self
            .terminals
            .lock()
            .unwrap()
            .get_mut(args.terminal_id.0.as_ref())
        {
            terminal
                .child
                .kill()
                .map_err(|e| acp::Error::internal_error().data(e.to_string()))?;
        }
        Ok(acp::KillTerminalResponse::new())
    }

    async fn release_terminal(
        &self,
        args: acp::ReleaseTerminalRequest,
    ) -> acp::Result<acp::ReleaseTerminalResponse> {
        if let Some(mut terminal) = self
            .terminals
            .lock()
            .unwrap()
            .remove(args.terminal_id.0.as_ref())
        {
            let _ = terminal.child.kill();
            let _ = terminal.child.wait();
        }
        Ok(acp::ReleaseTerminalResponse::new())
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
            acp::SessionUpdate::UsageUpdate(u) => {
                self.emit(AgentEvent::ContextUsage {
                    used: u.used,
                    size: u.size,
                });
            }
            _ => {}
        }
        Ok(())
    }
}

fn update_exit(terminal: &mut LocalTerminal) -> acp::Result<()> {
    if terminal.exit_code.is_none()
        && let Some(status) = terminal
            .child
            .try_wait()
            .map_err(|e| acp::Error::internal_error().data(e.to_string()))?
    {
        terminal.exit_code = Some(status.code().unwrap_or(1).max(0) as u32);
    }
    Ok(())
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
                        UiCommand::Prompt { .. } => {
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

    // Bring User/Machine `env_key` values into this process so the child agent
    // can advertise `xai.api_key` for config.toml BYOK models (e.g. Qwen).
    let hydrated = crate::config_io::hydrate_model_env_keys();
    if hydrated > 0 {
        tracing::info!(hydrated, "injected model env_keys before agent spawn");
    }

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
        terminals: Arc::new(Mutex::new(HashMap::new())),
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
                        .terminal(true),
                )
                .client_info(
                    acp::Implementation::new("bony-build", env!("CARGO_PKG_VERSION"))
                        .title("Bony Build"),
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
            let tail = stderr_buf
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
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

    let has_xai_creds = std::path::Path::new(&std::env::var_os("USERPROFILE").unwrap_or_default())
        .join(".grok")
        .join("auth.json")
        .is_file()
        || std::env::var_os("XAI_API_KEY").is_some()
        || std::env::var_os("GROK_CODE_XAI_API_KEY").is_some();
    let has_byok = crate::config_io::has_usable_model_credentials();
    let has_noninteractive_auth = method_ids
        .iter()
        .any(|id| id == "cached_token" || id == "xai.api_key");

    if !has_xai_creds && !has_byok && !has_noninteractive_auth {
        let _ = child.kill().await;
        emit(AgentEvent::NeedsLogin {
            message: "Sign in, or configure any LLM in ~/.grok/config.toml ([model.*] + api_key/env_key) and restart.".into(),
        });
        // Wait for Login / Shutdown before reconnecting.
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                UiCommand::Shutdown => return SessionEnd::Shutdown,
                UiCommand::Login => {
                    run_grok_login(config, event_tx, egui_ctx).await;
                    return SessionEnd::Reconnect;
                }
                UiCommand::Prompt { .. } => {
                    emit(AgentEvent::NeedsLogin {
                        message: "Please sign in first, then send a message.".into(),
                    });
                }
                _ => {}
            }
        }
        return SessionEnd::Shutdown;
    }

    if let Some(method) = select_auth_method(&init.auth_methods, default_id.as_deref(), has_byok) {
        let id = method.id().0.to_string();
        // Don't force browser login when config.toml BYOK models are ready.
        let interactive = id == "grok.com" || id.contains("oidc");
        if !(interactive && has_byok && !has_xai_creds) {
            emit(AgentEvent::Status(format!("Signing in ({id})…")));
            let mut req = acp::AuthenticateRequest::new(method.id().clone());
            if !interactive {
                req = req.meta(serde_json::json!({ "headless": true }).as_object().cloned());
            }
            if let Err(e) = conn.authenticate(req).await {
                if has_byok {
                    tracing::warn!(error = %e, "auth failed; continuing with config.toml models");
                } else {
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
                            UiCommand::Prompt { .. } => {
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
        } else {
            tracing::info!("skipping interactive login; using config.toml model credentials");
            emit(AgentEvent::Status("Using config.toml models…".into()));
        }
    }

    emit(AgentEvent::Status("Opening session…".into()));
    let catalog = crate::config_io::load_models_catalog();
    let mut session_req = acp::NewSessionRequest::new(config.cwd.clone()).mcp_servers(vec![]);
    if let Some(model_id) = catalog.default_id.as_ref() {
        session_req = session_req.meta(
            serde_json::json!({ "modelId": model_id })
                .as_object()
                .cloned(),
        );
    }
    let restored = config.resume_session_id.is_some();
    let (session_id, current_model_id, current_model_name, mut models, current_mode_id, modes) =
        if let Some(saved_id) = config.resume_session_id.as_ref() {
            match conn
                .load_session(acp::LoadSessionRequest::new(
                    acp::SessionId::new(saved_id.clone()),
                    config.cwd.clone(),
                ))
                .await
            {
                Ok(s) => {
                    let (model_id, model_name, models) =
                        extract_model_state(s.models.as_ref(), &catalog);
                    let (mode_id, modes) = extract_modes(s.modes.as_ref());
                    (
                        acp::SessionId::new(saved_id.clone()),
                        model_id,
                        model_name,
                        models,
                        mode_id,
                        modes,
                    )
                }
                Err(e) => {
                    let _ = child.kill().await;
                    return SessionEnd::Fatal(format!("session/load failed for {saved_id}: {e}"));
                }
            }
        } else {
            match conn.new_session(session_req).await {
                Ok(s) => {
                    let (model_id, model_name, models) = extract_models(&s);
                    let (mode_id, modes) = extract_modes(s.modes.as_ref());
                    (s.session_id, model_id, model_name, models, mode_id, modes)
                }
                Err(e) => {
                    let _ = child.kill().await;
                    return SessionEnd::Fatal(format!(
                        "session/new failed: {e}. Try Sign in again."
                    ));
                }
            }
        };
    emit(AgentEvent::Connected {
        session_id: session_id.0.to_string(),
        cwd: config.cwd.clone(),
        current_model_id,
        current_model_name,
        models: models.clone(),
        current_mode_id,
        modes,
        restored,
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
            UiCommand::SetMode { mode_id } => {
                match conn
                    .set_session_mode(acp::SetSessionModeRequest::new(
                        session_id.clone(),
                        acp::SessionModeId::new(mode_id.clone()),
                    ))
                    .await
                {
                    Ok(_) => emit(AgentEvent::ModeChanged { mode_id }),
                    Err(e) => emit(AgentEvent::Error(format!("切换模式失败: {e}"))),
                }
            }
            UiCommand::Cancel => {
                let _ = conn
                    .cancel(acp::CancelNotification::new(session_id.clone()))
                    .await;
            }
            UiCommand::PermissionResponse { option_id } => {
                if let Some(pending) = pending.lock().unwrap().take() {
                    let _ = pending.respond.send(option_id);
                }
            }
            UiCommand::Prompt { text, attachments } => {
                let text = text.trim().to_string();
                if text.is_empty() && attachments.is_empty() {
                    continue;
                }
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(acp::ContentBlock::Text(acp::TextContent::new(text)));
                }
                for attachment in attachments {
                    if attachment.mime_type.starts_with("image/") {
                        use base64::Engine as _;
                        blocks.push(acp::ContentBlock::Image(acp::ImageContent::new(
                            base64::engine::general_purpose::STANDARD.encode(attachment.data),
                            attachment.mime_type,
                        )));
                    } else {
                        let body = String::from_utf8_lossy(&attachment.data);
                        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(format!(
                            "\n<attachment name=\"{}\">\n{}\n</attachment>",
                            attachment.name, body
                        ))));
                    }
                }
                emit(AgentEvent::Status("Thinking…".into()));
                match conn
                    .prompt(acp::PromptRequest::new(session_id.clone(), blocks))
                    .await
                {
                    Ok(resp) => {
                        let usage = parse_usage_from_meta(resp.meta.as_ref());
                        emit(AgentEvent::TurnDone {
                            stop_reason: format!("{:?}", resp.stop_reason),
                            usage,
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
        std::process::Command::new(&grok_bin).arg("login").status()
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
    prefer_byok: bool,
) -> Option<&'a acp::AuthMethod> {
    if methods.is_empty() {
        return None;
    }
    if let Some(id) = default_id
        && let Some(m) = methods.iter().find(|m| m.id().0.as_ref() == id)
    {
        // When BYOK models are configured, don't honor a grok.com default.
        let id = m.id().0.as_ref();
        if !(prefer_byok && (id == "grok.com" || id.contains("oidc"))) {
            return Some(m);
        }
    }
    let noninteractive = methods
        .iter()
        .find(|m| m.id().0.as_ref() == "xai.api_key")
        .or_else(|| methods.iter().find(|m| m.id().0.as_ref() == "cached_token"));
    if noninteractive.is_some() {
        return noninteractive;
    }
    if prefer_byok {
        // Caller will skip interactive login and open a BYOK session.
        return None;
    }
    methods
        .iter()
        .find(|m| m.id().0.as_ref() == "grok.com")
        .or_else(|| methods.first())
}

fn extract_models(session: &acp::NewSessionResponse) -> (String, String, Vec<ModelChoice>) {
    extract_model_state(
        session.models.as_ref(),
        &crate::config_io::load_models_catalog(),
    )
}

fn extract_model_state(
    state: Option<&acp::SessionModelState>,
    catalog: &crate::config_io::ConfigModels,
) -> (String, String, Vec<ModelChoice>) {
    if let Some(state) = state {
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
    let id = catalog.default_id.clone().unwrap_or_default();
    let name = catalog
        .models
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "未设置".into());
    (id, name, catalog.models.clone())
}

fn extract_modes(state: Option<&acp::SessionModeState>) -> (String, Vec<ModeChoice>) {
    let Some(state) = state else {
        return (String::new(), Vec::new());
    };
    (
        state.current_mode_id.0.to_string(),
        state
            .available_modes
            .iter()
            .map(|m| ModeChoice {
                id: m.id.0.to_string(),
                name: m.name.clone(),
                description: m.description.clone().unwrap_or_default(),
            })
            .collect(),
    )
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
