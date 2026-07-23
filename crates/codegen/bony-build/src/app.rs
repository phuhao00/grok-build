//! Codex-style shell: left task sidebar, main chat, floating composer.

use std::sync::mpsc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Vec2,
};
use tokio::sync::mpsc as tokio_mpsc;

use crate::agent_bridge::{self, BridgeConfig};
use crate::charts;
use crate::events::{AgentEvent, AttachmentPayload, UiCommand};
use crate::git_workspace::{ChangeKind, FileChange, GitWorkspaceService};
use crate::markdown;
use crate::model::{AppModel, MainNav, Role, TimelineItem, UsageTab};
use crate::task::{
    PermissionMode, SqliteTaskRepository, TaskRepository, TaskState, TaskStatus, unix_time,
};
use crate::unity::{
    CliStatus, EVAL_PRESETS, EditorLinkStatus, LoopPhase, PipelineStatus, SetupStep, StepState,
    UNITY_CHAT_CHIPS, UnityAction, UnityChatCmd, UnityState, compile_unity_scene_command,
    format_relative, parse_generated_unity_plan_unrestricted, parse_unity_chat_command,
    unity_chat_help_text, wants_unity_help,
};
use crate::usage::{aggregate_model_usage, format_tokens, remember_project};

const BG: Color32 = Color32::from_rgb(22, 22, 24);
const SIDEBAR: Color32 = Color32::from_rgb(18, 18, 20);
const PANEL: Color32 = Color32::from_rgb(32, 32, 36);
const PANEL_2: Color32 = Color32::from_rgb(40, 40, 46);
const BORDER: Color32 = Color32::from_rgb(55, 55, 62);
const TEXT: Color32 = Color32::from_rgb(236, 236, 240);
const MUTED: Color32 = Color32::from_rgb(148, 150, 160);
const ACCENT: Color32 = Color32::from_rgb(245, 245, 247);
const USER_BG: Color32 = Color32::from_rgb(48, 52, 64);
const ASSIST_BG: Color32 = Color32::from_rgb(28, 28, 32);
const TOOL_BG: Color32 = Color32::from_rgb(30, 30, 34);
const DANGER: Color32 = Color32::from_rgb(220, 90, 90);
const OK: Color32 = Color32::from_rgb(110, 190, 130);
const ACCENT_BAR: Color32 = Color32::from_rgb(90, 140, 255);
const AVATAR: Color32 = Color32::from_rgb(70, 120, 220);
const SELECTED: Color32 = Color32::from_rgb(42, 44, 52);
const UNITY_ACCENT: Color32 = Color32::from_rgb(0, 180, 216);
const MAX_CHAT_W: f32 = 860.0;
const SIDEBAR_W: f32 = 248.0;
const RIGHT_PANEL_W: f32 = 280.0;
const TITLE_BAR_H: f32 = 36.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TaskListFilter {
    #[default]
    All,
    Active,
    WaitingApproval,
    Completed,
    Failed,
}

#[derive(Clone)]
struct PendingUnityApproval {
    summary: String,
    csharp: String,
    risks: Vec<String>,
}

impl TaskListFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Active => "进行中",
            Self::WaitingApproval => "待审批",
            Self::Completed => "已完成",
            Self::Failed => "失败",
        }
    }

    fn matches(self, status: TaskStatus) -> bool {
        match self {
            Self::All => true,
            Self::Active => matches!(status, TaskStatus::Draft | TaskStatus::Running),
            Self::WaitingApproval => status == TaskStatus::WaitingApproval,
            Self::Completed => status == TaskStatus::Completed,
            Self::Failed => status == TaskStatus::Failed,
        }
    }
}

const STARTERS: &[&str] = &[
    "解释这个代码库的结构",
    "找出最近改动里可能的 bug",
    "给主 agent 循环补测试",
    "总结认证是怎么工作的",
];

pub struct BonyBuildApp {
    model: AppModel,
    event_rx: mpsc::Receiver<AgentEvent>,
    event_tx: mpsc::Sender<AgentEvent>,
    cmd_tx: Option<tokio_mpsc::UnboundedSender<UiCommand>>,
    started: bool,
    config: BridgeConfig,
    task_repo: Option<SqliteTaskRepository>,
    tasks: Vec<TaskState>,
    active_task_id: Option<String>,
    attachments: Vec<AttachmentPayload>,
    changes: Vec<FileChange>,
    selected_diff: Option<(std::path::PathBuf, String)>,
    task_error: Option<String>,
    pending_git_action: Option<(bool, std::path::PathBuf)>,
    task_list_filter: TaskListFilter,
    rename_task: Option<(String, String)>,
    delete_task: Option<String>,
    unity: UnityState,
    /// Latest operation id before a Unity action launched from chat.
    pending_unity_chat: Option<u64>,
    pending_unity_planner: bool,
    pending_unity_approval: Option<PendingUnityApproval>,
    /// Composer shows Unity quick-control chips (local CLI, not agent).
    unity_chat_mode: bool,
    /// After soft cancel, next Stop click force-kills the agent.
    stop_armed_force: bool,
}

impl BonyBuildApp {
    pub fn new(cc: &eframe::CreationContext<'_>, mut config: BridgeConfig) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        configure_style(&cc.egui_ctx);
        let (event_tx, event_rx) = mpsc::channel();
        let task_repo = SqliteTaskRepository::open_default().ok();
        let mut tasks = task_repo
            .as_ref()
            .and_then(|r| r.list(false).ok())
            .unwrap_or_default();
        let mut active = tasks
            .iter()
            .find(|t| t.project_path == config.cwd && t.session_id.is_some())
            .cloned();
        let mut init_error = None;
        if active.is_none() {
            let project = GitWorkspaceService::primary_repo_root(&config.cwd)
                .ok()
                .flatten()
                .unwrap_or_else(|| config.cwd.clone());
            let mut task = TaskState::draft(project.clone(), String::new());
            match GitWorkspaceService::create_worktree(&project, &task.id, &task.title) {
                Ok(worktree) => {
                    task.worktree_path = worktree.path;
                    task.branch = Some(worktree.branch);
                    task.isolated = true;
                }
                Err(e)
                    if GitWorkspaceService::repo_root(&project)
                        .ok()
                        .flatten()
                        .is_some() =>
                {
                    init_error = Some(format!(
                        "初始任务无法创建 worktree：{e}。当前任务使用共享目录，发送前请确认。"
                    ))
                }
                Err(_) => {}
            }
            if let Some(repo) = &task_repo {
                let _ = repo.save(&task);
            }
            tasks.insert(0, task.clone());
            active = Some(task);
        }
        if let Some(task) = active.as_ref() {
            config.cwd = task.worktree_path.clone();
            config.resume_session_id = task.session_id.clone();
        }
        let active_task_id = active.as_ref().map(|t| t.id.clone());
        Self {
            model: AppModel::new(config.cwd.clone()),
            event_rx,
            event_tx,
            cmd_tx: None,
            started: false,
            config,
            task_repo,
            tasks,
            active_task_id,
            attachments: Vec::new(),
            changes: Vec::new(),
            selected_diff: None,
            task_error: init_error,
            pending_git_action: None,
            task_list_filter: TaskListFilter::All,
            rename_task: None,
            delete_task: None,
            unity: UnityState::default(),
            pending_unity_chat: None,
            pending_unity_planner: false,
            pending_unity_approval: None,
            unity_chat_mode: false,
            stop_armed_force: false,
        }
    }

    fn ensure_started(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        let cmd_tx =
            agent_bridge::spawn_bridge(self.config.clone(), ctx.clone(), self.event_tx.clone());
        self.cmd_tx = Some(cmd_tx);
    }

    /// Shut down the agent and reconnect against a new working directory.
    fn switch_project(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        let same = self
            .config
            .cwd
            .canonicalize()
            .ok()
            .zip(path.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
        if same {
            self.model.go_chat();
            return;
        }
        self.send_cmd(UiCommand::Shutdown);
        self.cmd_tx = None;
        self.config.cwd = path.clone();
        self.config.resume_session_id = None;
        self.active_task_id = None;
        remember_project(&mut self.model.recent_projects, &path);
        self.model.cwd = Some(path);
        self.model.connected = false;
        self.model.session_id = None;
        self.model.needs_login = false;
        self.model.status = "Connecting…".into();
        self.model.new_task();
        self.model.usage = crate::usage::SessionUsageState::default();
        let cmd_tx =
            agent_bridge::spawn_bridge(self.config.clone(), ctx.clone(), self.event_tx.clone());
        self.cmd_tx = Some(cmd_tx);
        self.started = true;
    }

    fn pick_project(&mut self, ctx: &egui::Context) {
        let start = self
            .model
            .cwd
            .clone()
            .unwrap_or_else(|| self.config.cwd.clone());
        if let Some(path) = rfd::FileDialog::new()
            .set_title("选择项目文件夹")
            .set_directory(start)
            .pick_folder()
        {
            self.switch_project(ctx, path);
        }
    }

    /// Pick a Unity project root for the Unity CLI panel only (does not switch agent cwd).
    fn pick_unity_project(&mut self, _ctx: &egui::Context) {
        let start = if self.unity.project_path.is_dir() {
            self.unity.project_path.clone()
        } else {
            self.config.cwd.clone()
        };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("选择 Unity 工程根目录（含 Assets）")
            .set_directory(start)
            .pick_folder()
        {
            self.unity.set_project_path(path);
            if crate::unity::is_unity_project_root(&self.unity.project_path) {
                self.unity.toast = Some("已绑定 Unity 工程，可继续安装 Pipeline / 探测".into());
            } else {
                self.unity.toast = Some(
                    "所选目录不像 Unity 工程根：请选含 Assets + ProjectSettings 的文件夹".into(),
                );
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            let finish_unity_planner = self.pending_unity_planner
                && matches!(&ev, AgentEvent::TurnDone { .. } | AgentEvent::Error(_));
            if matches!(
                ev,
                AgentEvent::TurnDone { .. }
                    | AgentEvent::Error(_)
                    | AgentEvent::Disconnected
                    | AgentEvent::NeedsLogin { .. }
                    | AgentEvent::Connected { .. }
            ) {
                self.stop_armed_force = false;
            }
            self.persist_event(&ev);
            self.model.apply(ev);
            if finish_unity_planner {
                self.finish_unity_planner();
            }
        }
    }

    fn finish_unity_planner(&mut self) {
        self.pending_unity_planner = false;
        let raw = self.model.latest_assistant_text();
        match parse_generated_unity_plan_unrestricted(&raw) {
            Ok((summary, csharp, risks)) => {
                let mode = self
                    .active_task_id
                    .as_ref()
                    .and_then(|id| self.tasks.iter().find(|task| &task.id == id))
                    .map(|task| task.permission_mode)
                    .unwrap_or(PermissionMode::Ask);
                if mode == PermissionMode::ReadOnly {
                    self.model.replace_latest_assistant(format!(
                        "Unity 计划已生成：{summary}\n\n当前任务是只读模式，没有执行修改。"
                    ));
                } else if mode == PermissionMode::Ask || !risks.is_empty() {
                    self.model
                        .replace_latest_assistant(format!("Unity 计划等待批准：{summary}"));
                    self.pending_unity_approval = Some(PendingUnityApproval {
                        summary,
                        csharp,
                        risks,
                    });
                    self.model.busy = false;
                    self.model.status = "等待 Unity 权限确认".into();
                } else {
                    self.execute_unity_plan(summary, csharp);
                }
            }
            Err(error) => {
                self.model.replace_latest_assistant(format!(
                    "无法生成可执行的 Unity 计划：{error}\n\n没有执行任何编辑器操作。"
                ));
                self.model.busy = false;
                self.model.status = "Unity 计划被拒绝".into();
            }
        }
    }

    fn execute_unity_plan(&mut self, summary: String, csharp: String) {
        self.model
            .replace_latest_assistant(format!("Unity 计划已批准：{summary}\n\n正在执行…"));
        self.unity.eval_input = csharp;
        self.pending_unity_chat = Some(self.unity.latest_record_id());
        self.model.busy = true;
        self.model.status = "正在执行 Unity 计划…".into();
        self.unity.run_action(UnityAction::Eval);
    }

    fn persist_event(&mut self, event: &AgentEvent) {
        let Some(id) = self.active_task_id.clone() else {
            return;
        };
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let mut soft_cancel_restored = false;
        match event {
            AgentEvent::Connected {
                session_id,
                current_model_id,
                restored,
                ..
            } => {
                task.session_id = Some(session_id.clone());
                task.model_id = current_model_id.clone();
                task.status = TaskStatus::Draft;
                if *restored {
                    // Soft-cancel only — ForceStop here caused an infinite reconnect loop
                    // because resume_session_id stays set and every Connected re-triggers it.
                    self.model.busy = false;
                    self.stop_armed_force = false;
                    self.model.status = "会话已恢复".into();
                    soft_cancel_restored = true;
                    // Next reconnects in this process should not look like a fresh restore.
                    self.config.resume_session_id = None;
                }
            }
            AgentEvent::PermissionRequest { .. } => task.status = TaskStatus::WaitingApproval,
            AgentEvent::TurnDone { .. } => {
                task.status = TaskStatus::Completed;
                self.changes =
                    GitWorkspaceService::changes(&task.worktree_path).unwrap_or_default();
            }
            AgentEvent::Error(_) => task.status = TaskStatus::Failed,
            _ => return,
        }
        task.updated_at = unix_time();
        if let Some(repo) = &self.task_repo {
            let _ = repo.save(task);
        }
        if soft_cancel_restored {
            self.send_cmd(UiCommand::Cancel);
        }
    }

    fn create_task(&mut self, ctx: &egui::Context) {
        let project = self
            .model
            .cwd
            .clone()
            .unwrap_or_else(|| self.config.cwd.clone());
        let project = GitWorkspaceService::primary_repo_root(&project)
            .ok()
            .flatten()
            .unwrap_or(project);
        let mut task = TaskState::draft(project.clone(), self.model.current_model_id.clone());
        match GitWorkspaceService::create_worktree(&project, &task.id, &task.title) {
            Ok(w) => {
                task.worktree_path = w.path;
                task.branch = Some(w.branch);
                task.isolated = true;
            }
            Err(e)
                if GitWorkspaceService::repo_root(&project)
                    .ok()
                    .flatten()
                    .is_some() =>
            {
                self.task_error = Some(format!(
                    "无法创建隔离 worktree：{e}\n任务未创建，避免静默共享工作目录。"
                ));
                return;
            }
            Err(_) => {}
        }
        if let Some(repo) = &self.task_repo {
            if let Err(e) = repo.save(&task) {
                self.task_error = Some(e);
                return;
            }
        }
        self.tasks.insert(0, task.clone());
        self.activate_task(ctx, task);
    }

    fn activate_task(&mut self, ctx: &egui::Context, task: TaskState) {
        self.send_cmd(UiCommand::ForceStop);
        self.send_cmd(UiCommand::Shutdown);
        self.cmd_tx = None;
        self.active_task_id = Some(task.id.clone());
        self.config.cwd = task.worktree_path.clone();
        self.config.resume_session_id = task.session_id.clone();
        self.model = AppModel::new(task.worktree_path.clone());
        self.model.task_title = task.title;
        self.model.busy = false;
        self.stop_armed_force = false;
        self.attachments.clear();
        self.changes.clear();
        self.selected_diff = None;
        let tx =
            agent_bridge::spawn_bridge(self.config.clone(), ctx.clone(), self.event_tx.clone());
        self.cmd_tx = Some(tx);
        self.started = true;
    }

    fn send_cmd(&self, cmd: UiCommand) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    fn send_prompt(&mut self) {
        let text = self.model.draft.trim().to_string();
        if self.try_send_unity_chat_command(&text) {
            self.model.draft.clear();
            return;
        }
        if (text.is_empty() && self.attachments.is_empty())
            || self.model.busy
            || self.model.needs_login
            || !self.model.connected
        {
            return;
        }
        if let Some(id) = self.active_task_id.clone()
            && let Some(task) = self.tasks.iter_mut().find(|t| t.id == id)
        {
            if task.title == "新任务" && !text.is_empty() {
                task.title = text.chars().take(42).collect();
                self.model.task_title = task.title.clone();
            }
            task.status = TaskStatus::Running;
            task.updated_at = unix_time();
            if let Some(repo) = &self.task_repo {
                let _ = repo.save(task);
            }
        }
        self.model.draft.clear();
        self.model.push_user(if text.is_empty() {
            format!("已附加 {} 个文件", self.attachments.len())
        } else {
            text.clone()
        });
        let attachments = std::mem::take(&mut self.attachments);
        self.send_cmd(UiCommand::Prompt { text, attachments });
    }

    fn try_send_unity_chat_command(&mut self, text: &str) -> bool {
        if text.is_empty() || !self.attachments.is_empty() {
            return false;
        }
        if wants_unity_help(text) {
            self.model.push_local_user(text.to_string());
            self.model.push_local_assistant(unity_chat_help_text());
            self.unity_chat_mode = true;
            return true;
        }
        if let Some((label, eval)) = compile_unity_scene_command(text) {
            self.model.go_chat();
            self.unity_chat_mode = true;
            self.model.push_local_user(text.to_string());
            if self.unity.busy || self.unity.is_guiding() {
                self.model
                    .push_local_assistant("Unity 正在执行其他操作，请完成后再试。".into());
                return true;
            }
            let mode = self
                .active_task_id
                .as_ref()
                .and_then(|id| self.tasks.iter().find(|task| &task.id == id))
                .map(|task| task.permission_mode)
                .unwrap_or(PermissionMode::Ask);
            if mode == PermissionMode::ReadOnly {
                self.model
                    .push_local_assistant("当前任务是只读模式，没有执行 Unity 场景修改。".into());
                return true;
            }
            if mode == PermissionMode::Ask {
                self.pending_unity_approval = Some(PendingUnityApproval {
                    summary: label.clone(),
                    csharp: eval,
                    risks: Vec::new(),
                });
                self.model.busy = false;
                self.model.status = "等待 Unity 权限确认".into();
                return true;
            }
            self.unity.eval_input = eval;
            self.pending_unity_chat = Some(self.unity.latest_record_id());
            self.model.status = format!("正在控制 Unity：{label}");
            self.unity.run_action(UnityAction::Eval);
            return true;
        }
        let Some(cmd) = parse_unity_chat_command(text) else {
            if self.unity_chat_mode {
                if !self.model.connected || self.model.needs_login {
                    self.model.push_local_user(text.to_string());
                    self.model.push_local_assistant(
                        "通用 Unity 操作需要 Agent 生成结构化计划，请先连接 Agent。".into(),
                    );
                    return true;
                }
                self.model.push_user(text.to_string());
                self.pending_unity_planner = true;
                let planner_prompt = format!(
                    "You are a Unity Editor action compiler. Convert the user's request below into one Unity C# Eval body. Do not call tools. Output ONLY one JSON object with string fields `summary` and `csharp`, without markdown. The C# runs inside the open Unity Editor and must use UnityEngine/UnityEditor APIs, support Undo for mutations, mark changed scenes/assets dirty, and end with a return value. Never use filesystem, network, processes, environment variables, native interop, reflection, or shell APIs. User request: {}",
                    text
                );
                self.send_cmd(UiCommand::Prompt {
                    text: planner_prompt,
                    attachments: Vec::new(),
                });
                return true;
            }
            return false;
        };
        self.dispatch_unity_chat_cmd(cmd, Some(text));
        true
    }

    fn dispatch_unity_chat_cmd(&mut self, cmd: &UnityChatCmd, spoken: Option<&str>) {
        let label = spoken.unwrap_or(cmd.chip).to_string();
        self.model.go_chat();
        self.unity_chat_mode = true;
        self.model.push_local_user(label);
        if self.unity.busy || self.unity.is_guiding() {
            self.model
                .push_local_assistant("Unity 正在执行其他操作，请完成后再试。".into());
            return;
        }
        if let Some(expression) = cmd.eval {
            self.unity.eval_input = expression.to_string();
        }
        self.pending_unity_chat = Some(self.unity.latest_record_id());
        self.unity.run_action(cmd.action);
    }

    fn pick_attachments(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("添加上下文文件")
            .pick_files()
        else {
            return;
        };
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() > 10 * 1024 * 1024 {
                self.task_error = Some(format!("附件超过 10 MB：{}", path.display()));
                continue;
            }
            let ext = path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "js" | "ts" | "tsx"
                | "jsx" | "py" | "go" | "java" | "c" | "cpp" | "h" => "text/plain",
                _ => {
                    self.task_error = Some(format!("暂不支持的附件类型：{}", path.display()));
                    continue;
                }
            };
            if let Ok(data) = std::fs::read(&path) {
                self.attachments.push(AttachmentPayload {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    mime_type: mime.into(),
                    data,
                });
            }
        }
    }

    fn send_starter(&mut self, text: &str) {
        if self.model.busy || !self.model.connected || self.model.needs_login {
            return;
        }
        self.model.draft.clear();
        self.model.push_user(text.to_string());
        self.send_cmd(UiCommand::Prompt {
            text: text.to_string(),
            attachments: Vec::new(),
        });
    }

    /// Send machine context without rendering it as a giant user bubble.
    fn send_context_prompt(&mut self, display_text: &str, prompt: String) {
        if self.model.busy || !self.model.connected || self.model.needs_login {
            return;
        }
        self.model.draft.clear();
        self.model.push_user(display_text.to_string());
        self.send_cmd(UiCommand::Prompt {
            text: prompt,
            attachments: Vec::new(),
        });
    }
}

impl eframe::App for BonyBuildApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_started(ctx);
        self.drain_events();
        self.unity.ensure_detecting();
        // Only auto-bind agent cwd when it actually is (or sits inside) a Unity project.
        // Task worktrees like ~/.bony-worktrees/.../task-* must NOT override a chosen Unity root.
        self.unity.consider_agent_cwd(&self.config.cwd);
        self.unity.sync_setup_step();
        let unity_changed = self.unity.poll();
        if unity_changed || self.unity.needs_repaint() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        if self.pending_unity_chat.is_some()
            && unity_changed
            && !self.unity.busy
            && !self.unity.is_guiding()
        {
            let previous_id = self.pending_unity_chat.take().unwrap_or_default();
            self.model
                .push_local_assistant(self.unity.latest_chat_result_since(previous_id));
        }
        if let Some(toast) = self.unity.take_toast() {
            self.model.status = toast;
        }

        if self.model.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }

        self.title_bar(ctx);

        if self.model.show_left_sidebar {
            egui::SidePanel::left("codex_sidebar")
                .exact_width(SIDEBAR_W)
                .resizable(false)
                .frame(Frame::NONE.fill(SIDEBAR).inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 10,
                    bottom: 12,
                }))
                .show(ctx, |ui| {
                    self.sidebar(ui, ctx);
                });
        }

        if self.model.show_right_panel {
            egui::SidePanel::right("codex_right")
                .exact_width(RIGHT_PANEL_W)
                .resizable(false)
                .frame(
                    Frame::NONE
                        .fill(SIDEBAR)
                        .inner_margin(Margin::symmetric(14, 14))
                        .stroke(Stroke::new(1.0, BORDER)),
                )
                .show(ctx, |ui| {
                    self.right_panel(ui);
                });
        }

        let on_chat = self.model.main_nav == MainNav::Chat;
        let on_unity = self.model.main_nav == MainNav::Unity;
        let show_task_title =
            on_chat && (!self.model.is_empty_chat() || self.model.is_viewing_history());

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(0, 0)))
            .show(ctx, |ui| {
                // No second control row under the window buttons — title only when needed.
                if show_task_title || !on_chat {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        let title = if on_chat {
                            if self.model.task_title.contains("只读分析") {
                                "Unity 状态分析"
                            } else {
                                self.model.task_title.as_str()
                            }
                        } else {
                            self.model.main_nav.title()
                        };
                        ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
                        if on_unity {
                            ui.add_space(8.0);
                            let status_color = match self.unity.status {
                                CliStatus::Ready => OK,
                                CliStatus::Missing | CliStatus::Error => DANGER,
                                CliStatus::Checking | CliStatus::Unknown => MUTED,
                            };
                            ui.label(
                                RichText::new(self.unity.status.label())
                                    .size(12.0)
                                    .color(status_color),
                            );
                            if self.unity.demo_mode {
                                ui.label(
                                    RichText::new("· 演示模式").size(12.0).color(UNITY_ACCENT),
                                );
                            }
                        }
                    });
                }

                if on_chat {
                    let avail = ui.available_height();
                    let composer_h = if self.unity_chat_mode { 210.0 } else { 140.0 };
                    let chat_h = (avail - composer_h).max(120.0);

                    egui::ScrollArea::vertical()
                        .id_salt("chat_scroll")
                        .stick_to_bottom(self.model.auto_scroll)
                        .auto_shrink([false, false])
                        .max_height(chat_h)
                        .show(ui, |ui| {
                            centered_column(ui, |ui| {
                                if self.model.is_viewing_history() {
                                    ui.add_space(8.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            RichText::new("只读历史 · 发送新消息将回到当前会话")
                                                .size(12.0)
                                                .color(MUTED),
                                        );
                                    });
                                    ui.add_space(8.0);
                                }
                                if self.model.is_empty_chat() {
                                    self.empty_state(ui);
                                } else {
                                    self.timeline(ui);
                                }
                                if self.model.busy {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(RichText::new("处理中…").size(12.5).color(MUTED));
                                    });
                                }
                                ui.add_space(20.0);
                            });
                        });

                    ui.add_space(8.0);
                    centered_column(ui, |ui| {
                        if self.unity_chat_mode {
                            self.unity_composer_chips(ui);
                            ui.add_space(6.0);
                        }
                        self.floating_composer(ui);
                    });
                    ui.add_space(12.0);
                } else if on_unity {
                    egui::ScrollArea::vertical()
                        .id_salt("unity_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            centered_column(ui, |ui| {
                                self.unity_panel(ui);
                            });
                        });
                } else {
                    centered_column(ui, |ui| {
                        self.nav_placeholder(ui);
                    });
                }
            });

        self.user_menu_popup(ctx);
        self.usage_detail_window(ctx);
        self.permission_modal(ctx);
        self.unity_permission_modal(ctx);
        self.model_picker_modal(ctx);
        self.about_modal(ctx);
        self.rename_task_modal(ctx);
        self.delete_task_modal(ctx);
        self.task_error_modal(ctx);
        self.git_confirmation_modal(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send_cmd(UiCommand::Shutdown);
    }
}

impl BonyBuildApp {
    /// One-row Codex chrome: left controls + menus | drag | right toggle + window buttons.
    fn title_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("title_bar")
            .exact_height(TITLE_BAR_H)
            .frame(Frame::NONE.fill(SIDEBAR).inner_margin(Margin {
                left: 8,
                right: 0,
                top: 0,
                bottom: 0,
            }))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter()
                    .hline(full.x_range(), full.bottom(), Stroke::new(1.0, BORDER));

                ui.allocate_ui_with_layout(
                    Vec2::new(full.width(), TITLE_BAR_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        // —— Left cluster ——
                        let left_on = self.model.show_left_sidebar;
                        if panel_toggle_btn(ui, PanelSide::Left, "切换侧栏", left_on) {
                            self.model.show_left_sidebar = !left_on;
                        }
                        let can_back = self.model.is_viewing_history();
                        if nav_chevron_btn(ui, NavDir::Back, "返回当前会话", can_back) && can_back
                        {
                            self.model.return_to_live();
                        }
                        let _ = nav_chevron_btn(ui, NavDir::Forward, "前进", false);

                        ui.add_space(8.0);
                        ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
                        ui.visuals_mut().button_frame = false;
                        for (label, build) in
                            [("文件", 0u8), ("编辑", 1u8), ("视图", 2u8), ("帮助", 3u8)]
                        {
                            ui.menu_button(RichText::new(label).size(13.0).color(MUTED), |ui| {
                                ui.visuals_mut().button_frame = true;
                                match build {
                                    0 => {
                                        if ui.button("新建任务").clicked() {
                                            self.create_task(ctx);
                                            ui.close_menu();
                                        }
                                        if ui.button("打开项目…").clicked() {
                                            self.pick_project(ctx);
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("退出").clicked() {
                                            ui.close_menu();
                                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                        }
                                    }
                                    1 => {
                                        if ui.button("聚焦输入框").clicked() {
                                            self.model.go_chat();
                                            self.model.focus_composer = true;
                                            ui.close_menu();
                                        }
                                        if ui.button("清空草稿").clicked() {
                                            self.model.draft.clear();
                                            ui.close_menu();
                                        }
                                    }
                                    2 => {
                                        let left_label = if self.model.show_left_sidebar {
                                            "隐藏侧栏"
                                        } else {
                                            "显示侧栏"
                                        };
                                        if ui.button(left_label).clicked() {
                                            self.model.show_left_sidebar =
                                                !self.model.show_left_sidebar;
                                            ui.close_menu();
                                        }
                                        let right_label = if self.model.show_right_panel {
                                            "隐藏右侧栏"
                                        } else {
                                            "显示右侧栏"
                                        };
                                        if ui.button(right_label).clicked() {
                                            self.model.show_right_panel =
                                                !self.model.show_right_panel;
                                            ui.close_menu();
                                        }
                                        if ui.button("使用统计").clicked() {
                                            self.model.show_usage_detail = true;
                                            ui.close_menu();
                                        }
                                        if ui.button("Unity 控制").clicked() {
                                            self.model.main_nav = MainNav::Unity;
                                            self.unity_chat_mode = true;
                                            self.unity.ensure_detecting();
                                            ui.close_menu();
                                        }
                                        if ui.button("聊天里控制 Unity").clicked() {
                                            self.model.go_chat();
                                            self.unity_chat_mode = true;
                                            self.model.focus_composer = true;
                                            ui.close_menu();
                                        }
                                    }
                                    _ => {
                                        if ui.button("关于 Bony Build").clicked() {
                                            self.model.show_about = true;
                                            ui.close_menu();
                                        }
                                    }
                                }
                            });
                        }

                        // —— Right cluster (same row): panel toggle + window controls ——
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if win_chrome_btn(ui, WinChrome::Close) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                            if win_chrome_btn(
                                ui,
                                if maximized {
                                    WinChrome::Restore
                                } else {
                                    WinChrome::Maximize
                                },
                            ) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                            }
                            if win_chrome_btn(ui, WinChrome::Minimize) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }

                            ui.add_space(4.0);
                            let right_on = self.model.show_right_panel;
                            if panel_toggle_btn(
                                ui,
                                PanelSide::Right,
                                if right_on {
                                    "隐藏右侧栏"
                                } else {
                                    "显示右侧栏"
                                },
                                right_on,
                            ) {
                                self.model.show_right_panel = !right_on;
                            }

                            // Drag the empty middle.
                            let drag_rect = ui.available_rect_before_wrap();
                            let drag_resp = ui.interact(
                                drag_rect,
                                ui.id().with("title_drag"),
                                egui::Sense::click_and_drag(),
                            );
                            if drag_resp.drag_started_by(egui::PointerButton::Primary) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            }
                            if drag_resp.double_clicked() {
                                let maximized =
                                    ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                            }
                        });
                    },
                );
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Bony Build").size(16.0).strong().color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search_on = self.model.show_task_search;
                if search_icon_btn(ui, search_on) {
                    self.model.show_task_search = !search_on;
                    if !self.model.show_task_search {
                        self.model.task_filter.clear();
                    }
                }
            });
        });

        if self.model.show_task_search {
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.model.task_filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("筛选任务…")
                    .frame(true),
            );
        }

        ui.add_space(12.0);

        if nav_item(ui, "＋  新建任务", false) {
            self.create_task(ctx);
        }
        ui.add_space(2.0);
        let chat_selected =
            self.model.main_nav == MainNav::Chat && !self.model.is_viewing_history();
        if nav_item(ui, "⊕  聊天", chat_selected) {
            self.model.return_to_live();
        }
        ui.add_space(2.0);
        let unity_selected = self.model.main_nav == MainNav::Unity;
        if nav_item(ui, "◇  Unity 控制", unity_selected) {
            self.model.main_nav = MainNav::Unity;
            self.unity_chat_mode = true;
            self.unity.ensure_detecting();
        }
        ui.add_space(2.0);
        if ui
            .add(
                egui::Button::new(RichText::new("    在聊天里用指令控制 →").size(11.5).color(
                    if self.unity_chat_mode {
                        UNITY_ACCENT
                    } else {
                        MUTED
                    },
                ))
                .fill(Color32::TRANSPARENT)
                .frame(false),
            )
            .on_hover_text("打开聊天并显示 Unity 快捷指令（本地 CLI，不经 Agent）")
            .clicked()
        {
            self.model.go_chat();
            self.unity_chat_mode = true;
            self.model.focus_composer = true;
            if self.model.is_empty_chat() {
                self.model.push_local_assistant(unity_chat_help_text());
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("项目").size(12.0).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("＋").size(13.0).color(MUTED))
                            .fill(Color32::TRANSPARENT)
                            .frame(false),
                    )
                    .on_hover_text("打开项目")
                    .clicked()
                {
                    self.pick_project(ctx);
                }
            });
        });
        ui.add_space(4.0);

        let projects = self.model.recent_projects.clone();
        let current = self.model.cwd.clone();
        if projects.is_empty() {
            ui.label(RichText::new("没有项目").size(12.0).color(MUTED));
        } else {
            for path in &projects {
                let label = AppModel::project_label(path);
                let selected = current
                    .as_ref()
                    .and_then(|c| c.canonicalize().ok())
                    .zip(path.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b)
                    || current.as_ref() == Some(path);
                if nav_item(ui, &format!("📁  {label}"), selected) {
                    self.switch_project(ctx, path.clone());
                }
                ui.add_space(1.0);
            }
        }

        ui.add_space(14.0);
        ui.label(RichText::new("任务").size(12.0).color(MUTED));
        ui.add_space(6.0);

        egui::ComboBox::from_id_salt("task_status_filter")
            .selected_text(self.task_list_filter.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for filter in [
                    TaskListFilter::All,
                    TaskListFilter::Active,
                    TaskListFilter::WaitingApproval,
                    TaskListFilter::Completed,
                    TaskListFilter::Failed,
                ] {
                    ui.selectable_value(&mut self.task_list_filter, filter, filter.label());
                }
            });
        ui.add_space(6.0);

        let filter = self.model.task_filter.trim().to_lowercase();
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| self.task_list_filter.matches(t.status))
            .filter(|t| filter.is_empty() || t.title.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        let active_id = self.active_task_id.clone();

        egui::ScrollArea::vertical()
            .id_salt("task_list")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if tasks.is_empty() {
                    ui.label(
                        RichText::new(if self.model.task_filter.trim().is_empty() {
                            "还没有任务记录"
                        } else {
                            "没有匹配的任务"
                        })
                        .size(12.0)
                        .color(MUTED),
                    );
                }
                for task in &tasks {
                    let selected = active_id.as_ref() == Some(&task.id);
                    let fill = if selected {
                        SELECTED
                    } else {
                        Color32::TRANSPARENT
                    };
                    let resp = Frame::new()
                        .fill(fill)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(&task.title).size(13.0).color(if selected {
                                TEXT
                            } else {
                                MUTED
                            }));
                            ui.label(
                                RichText::new(format!(
                                    "{}{} · {}",
                                    if task.isolated { "隔离" } else { "共享" },
                                    task.branch
                                        .as_deref()
                                        .map(|b| format!(" · {b}"))
                                        .unwrap_or_default(),
                                    task.status.label()
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let mut archive = false;
                    let mut rename = false;
                    let mut request_delete = false;
                    resp.context_menu(|ui| {
                        if ui.button("重命名任务").clicked() {
                            rename = true;
                            ui.close_menu();
                        }
                        if ui.button("归档任务").clicked() {
                            archive = true;
                            ui.close_menu();
                        }
                        if ui.button("删除任务记录").clicked() {
                            request_delete = true;
                            ui.close_menu();
                        }
                    });
                    if resp.clicked() {
                        self.activate_task(ctx, task.clone());
                    }
                    if archive {
                        if let Some(found) = self.tasks.iter_mut().find(|t| t.id == task.id) {
                            found.status = TaskStatus::Archived;
                            found.updated_at = unix_time();
                            if let Some(repo) = &self.task_repo {
                                let _ = repo.save(found);
                            }
                        }
                        self.tasks.retain(|t| t.id != task.id);
                    }
                    if rename {
                        self.rename_task = Some((task.id.clone(), task.title.clone()));
                    }
                    if request_delete {
                        self.delete_task = Some(task.id.clone());
                    }
                    ui.add_space(2.0);
                }
            });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            let pill = Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(20))
                .inner_margin(Margin::symmetric(8, 6))
                .stroke(Stroke::new(1.0, BORDER))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        avatar_circle(ui, &self.model.initials());
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(&self.model.display_name)
                                .size(13.0)
                                .color(TEXT),
                        );
                    });
                })
                .response
                .interact(egui::Sense::click());
            if pill.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if pill.clicked() {
                self.model.show_user_menu = !self.model.show_user_menu;
            }
        });
    }

    fn rename_task_modal(&mut self, ctx: &egui::Context) {
        let Some((task_id, mut title)) = self.rename_task.take() else {
            return;
        };
        let mut keep_open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new("重命名任务")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut title)
                        .desired_width(f32::INFINITY)
                        .hint_text("任务名称"),
                );
                response.request_focus();
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(!title.trim().is_empty(), egui::Button::new("保存"))
                        .clicked()
                    {
                        save = true;
                    }
                });
            });
        if save {
            if let Some(task) = self.tasks.iter_mut().find(|task| task.id == task_id) {
                task.title = title.trim().chars().take(80).collect();
                task.updated_at = unix_time();
                if self.active_task_id.as_ref() == Some(&task.id) {
                    self.model.task_title = task.title.clone();
                }
                if let Some(repo) = &self.task_repo
                    && let Err(error) = repo.save(task)
                {
                    self.task_error = Some(error);
                }
            }
            keep_open = false;
        }
        if cancel {
            keep_open = false;
        }
        if keep_open {
            self.rename_task = Some((task_id, title));
        }
    }

    fn delete_task_modal(&mut self, ctx: &egui::Context) {
        let Some(task_id) = self.delete_task.take() else {
            return;
        };
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id).cloned() else {
            return;
        };
        let can_delete = self.active_task_id.as_ref() != Some(&task.id)
            && !matches!(
                task.status,
                TaskStatus::Running | TaskStatus::WaitingApproval
            );
        let mut keep_open = true;
        egui::Window::new("删除任务记录？")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(format!("将删除“{}”的本地任务索引。", task.title));
                if task.isolated {
                    ui.label(
                        RichText::new("不会自动删除 worktree 或其中未提交的修改。").color(MUTED),
                    );
                }
                if !can_delete {
                    ui.label(
                        RichText::new("请先切换到其他任务，并等待当前运行或审批结束。")
                            .color(DANGER),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        keep_open = false;
                    }
                    if ui
                        .add_enabled(
                            can_delete,
                            egui::Button::new(RichText::new("删除记录").color(DANGER)),
                        )
                        .clicked()
                    {
                        if let Some(repo) = &self.task_repo
                            && let Err(error) = repo.delete(&task.id)
                        {
                            self.task_error = Some(error);
                        }
                        self.tasks.retain(|item| item.id != task.id);
                        keep_open = false;
                    }
                });
            });
        if keep_open {
            self.delete_task = Some(task_id);
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("详情").size(15.0).strong().color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("✕").size(13.0).color(MUTED))
                            .fill(Color32::TRANSPARENT)
                            .frame(false),
                    )
                    .clicked()
                {
                    self.model.show_right_panel = false;
                }
            });
        });
        ui.add_space(12.0);

        let status = if self.model.needs_login {
            ("需要登录", DANGER)
        } else if self.model.status.contains("Error") {
            ("出错", DANGER)
        } else if self.model.busy {
            ("思考中…", MUTED)
        } else if self.model.connected {
            ("就绪", OK)
        } else {
            ("连接中…", MUTED)
        };
        ui.label(RichText::new("会话").size(12.0).color(MUTED));
        ui.label(RichText::new(status.0).size(14.0).color(status.1));
        ui.add_space(10.0);

        ui.label(RichText::new("工作目录").size(12.0).color(MUTED));
        let cwd = self
            .model
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".into());
        ui.label(RichText::new(cwd).size(12.5).color(TEXT));
        ui.add_space(10.0);

        ui.label(RichText::new("模型").size(12.0).color(MUTED));
        ui.label(
            RichText::new(&self.model.current_model_name)
                .size(13.5)
                .color(TEXT),
        );
        ui.add_space(10.0);

        let total = self.model.usage.cumulative.total_tokens;
        let hist: u64 = self
            .model
            .history_turns
            .iter()
            .map(|t| t.usage_delta.total_tokens)
            .sum();
        ui.label(RichText::new("Token").size(12.0).color(MUTED));
        ui.label(
            RichText::new(format!("Σ {}", format_tokens(total.max(hist))))
                .size(14.0)
                .strong()
                .color(TEXT),
        );
        ui.add_space(16.0);

        if ui
            .add(
                egui::Button::new(RichText::new("打开使用统计").size(13.0).color(TEXT))
                    .fill(PANEL_2)
                    .min_size(Vec2::new(ui.available_width(), 34.0))
                    .corner_radius(CornerRadius::same(8)),
            )
            .clicked()
        {
            self.model.show_usage_detail = true;
        }

        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Changes ({})", self.changes.len()))
                    .size(13.0)
                    .strong()
                    .color(TEXT),
            );
            if ui.small_button("刷新").clicked() {
                match GitWorkspaceService::changes(&self.config.cwd) {
                    Ok(v) => self.changes = v,
                    Err(e) => self.task_error = Some(e),
                }
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("changes")
            .max_height(220.0)
            .show(ui, |ui| {
                for change in self.changes.clone() {
                    let mark = match change.kind {
                        ChangeKind::Added => "A",
                        ChangeKind::Modified => "M",
                        ChangeKind::Deleted => "D",
                        ChangeKind::Renamed => "R",
                        ChangeKind::Untracked => "?",
                        ChangeKind::Conflicted => "!",
                    };
                    let label = format!("{mark}  {}", change.path.display());
                    if ui
                        .selectable_label(
                            self.selected_diff
                                .as_ref()
                                .is_some_and(|(p, _)| p == &change.path),
                            label,
                        )
                        .clicked()
                    {
                        match GitWorkspaceService::diff(&self.config.cwd, Some(&change.path), false)
                        {
                            Ok(diff) => self.selected_diff = Some((change.path.clone(), diff)),
                            Err(e) => self.task_error = Some(e),
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let action = if change.staged {
                            "取消暂存"
                        } else {
                            "暂存"
                        };
                        if ui.small_button(action).clicked() {
                            self.pending_git_action = Some((!change.staged, change.path.clone()));
                        }
                    });
                }
            });
        if let Some((path, diff)) = &self.selected_diff {
            ui.separator();
            ui.label(
                RichText::new(path.display().to_string())
                    .size(12.0)
                    .strong(),
            );
            egui::ScrollArea::both()
                .id_salt("diff_preview")
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.monospace(if diff.is_empty() {
                        "未跟踪文件暂无 diff"
                    } else {
                        diff
                    });
                });
        }
    }

    fn nav_placeholder(&mut self, ui: &mut egui::Ui) {
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(self.model.main_nav.title())
                    .size(22.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(self.model.main_nav.placeholder_blurb())
                    .size(14.0)
                    .color(MUTED),
            );
            ui.add_space(20.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("回到聊天").size(13.0).color(BG).strong())
                        .fill(ACCENT)
                        .min_size(Vec2::new(120.0, 34.0))
                        .corner_radius(CornerRadius::same(10)),
                )
                .clicked()
            {
                self.model.go_chat();
            }
        });
    }

    fn unity_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("对话或按钮都能驱动编辑器：观察 → 行动 → 验证")
                    .size(13.0)
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = self.unity.busy || self.unity.is_guiding();
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(
                            RichText::new("跑完整闭环").size(12.5).color(BG).strong(),
                        )
                        .fill(UNITY_ACCENT)
                        .corner_radius(CornerRadius::same(8)),
                    )
                    .on_hover_text("复现博文演示：观察禁用碰撞体 → 热修复 → Play 验证")
                    .clicked()
                {
                    self.unity.run_action(UnityAction::RunFullLoop);
                }
                if ui
                    .button(RichText::new("打开对话控制").size(12.0).color(UNITY_ACCENT))
                    .on_hover_text("回到聊天，用一句话或快捷芯片控制 Unity CLI")
                    .clicked()
                {
                    self.model.go_chat();
                    self.unity_chat_mode = true;
                    self.model.focus_composer = true;
                    if self.model.is_empty_chat() {
                        self.model.push_local_assistant(unity_chat_help_text());
                    }
                }
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(RichText::new("在聊天中分析").size(12.0)),
                    )
                    .on_hover_text("切换到聊天并给出简短诊断，不执行 Unity 命令")
                    .clicked()
                {
                    let briefing = self.unity.compact_chat_briefing();
                    self.model.go_chat();
                    self.send_context_prompt("分析当前 Unity 状态", briefing);
                }
            });
        });
        if let Some(guide) = &self.unity.guide_label {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(guide).size(12.5).color(UNITY_ACCENT));
            });
        }
        ui.add_space(12.0);

        self.unity_setup_wizard(ui);
        ui.add_space(14.0);
        self.unity_status_card(ui);
        ui.add_space(14.0);
        self.unity_pipeline_card(ui);
        ui.add_space(14.0);
        self.unity_scene_card(ui);
        ui.add_space(14.0);
        self.unity_loop_card(ui);
        ui.add_space(14.0);
        self.unity_actions_card(ui);
        ui.add_space(14.0);
        self.unity_history_card(ui);
        ui.add_space(24.0);
    }

    fn unity_setup_wizard(&mut self, ui: &mut egui::Ui) {
        let busy = self.unity.busy || self.unity.is_guiding();
        let focus = self.unity.focused_setup_step();
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.5, UNITY_ACCENT))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("引导执行")
                            .size(15.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "步骤 {}/{}",
                            self.unity.setup_step.index() + 1,
                            SetupStep::ALL.len()
                        ))
                        .size(12.0)
                        .color(UNITY_ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("跟随推荐步骤").clicked() {
                            self.unity.setup_focus = None;
                            self.unity.sync_setup_step();
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("按顺序完成：安装 CLI → 检测 → 确认项目 → Pipeline → 探测编辑器 → 闭环")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.add_space(10.0);

                // Step rail
                ui.horizontal_wrapped(|ui| {
                    for step in SetupStep::ALL {
                        let state = self.unity.step_state(step);
                        let selected = focus == step;
                        let (fill, stroke, text_color) = match state {
                            StepState::Done => (
                                Color32::from_rgb(36, 56, 44),
                                Stroke::new(1.0, OK),
                                OK,
                            ),
                            StepState::Current => (
                                PANEL_2,
                                Stroke::new(1.5, UNITY_ACCENT),
                                UNITY_ACCENT,
                            ),
                            StepState::Locked => (
                                PANEL,
                                Stroke::new(1.0, BORDER),
                                MUTED,
                            ),
                        };
                        let label = format!(
                            "{} {}",
                            match state {
                                StepState::Done => "✓",
                                StepState::Current => "●",
                                StepState::Locked => "○",
                            },
                            step.title()
                        );
                        let resp = ui.add(
                            egui::Button::new(RichText::new(label).size(11.5).color(text_color))
                                .fill(if selected { PANEL_2 } else { fill })
                                .stroke(if selected {
                                    Stroke::new(1.5, ACCENT)
                                } else {
                                    stroke
                                })
                                .corner_radius(CornerRadius::same(8))
                                .min_size(Vec2::new(0.0, 28.0)),
                        );
                        if resp.clicked() {
                            self.unity.setup_focus = Some(step);
                        }
                    }
                });

                ui.add_space(12.0);
                Frame::new()
                    .fill(PANEL_2)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(focus.title())
                                .size(14.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.add_space(4.0);
                        ui.label(RichText::new(focus.blurb()).size(12.5).color(MUTED));

                        match focus {
                            SetupStep::InstallCli => {
                                ui.add_space(8.0);
                                let hint = UnityState::install_hint();
                                ui.label(
                                    RichText::new(hint).size(11.5).monospace().color(TEXT),
                                );
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("① 复制安装命令")
                                                    .size(12.5)
                                                    .color(BG)
                                                    .strong(),
                                            )
                                            .fill(UNITY_ACCENT)
                                            .min_size(Vec2::new(0.0, 32.0))
                                            .corner_radius(CornerRadius::same(8)),
                                        )
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(hint.to_string());
                                        self.unity.advance_after_cli_install_copied();
                                    }
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            egui::Button::new(
                                                RichText::new("② 我已安装，重新检测")
                                                    .size(12.5)
                                                    .color(TEXT),
                                            )
                                            .fill(PANEL)
                                            .stroke(Stroke::new(1.0, BORDER))
                                            .min_size(Vec2::new(0.0, 32.0))
                                            .corner_radius(CornerRadius::same(8)),
                                        )
                                        .clicked()
                                    {
                                        self.unity.setup_focus = Some(SetupStep::DetectCli);
                                        self.unity.run_action(UnityAction::RefreshDetect);
                                    }
                                });
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(
                                        "在外部 PowerShell 粘贴执行安装脚本，完成后点②。安装过程可能需 1–2 分钟。",
                                    )
                                    .size(11.5)
                                    .color(MUTED),
                                );
                            }
                            SetupStep::DetectCli => {
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            egui::Button::new(
                                                RichText::new("重新检测 CLI")
                                                    .size(12.5)
                                                    .color(BG)
                                                    .strong(),
                                            )
                                            .fill(UNITY_ACCENT)
                                            .min_size(Vec2::new(0.0, 32.0))
                                            .corner_radius(CornerRadius::same(8)),
                                        )
                                        .clicked()
                                    {
                                        self.unity.run_action(UnityAction::RefreshDetect);
                                    }
                                    if self.unity.status == CliStatus::Ready {
                                        ui.label(
                                            RichText::new("已就绪，可进入下一步")
                                                .size(12.5)
                                                .color(OK),
                                        );
                                    }
                                });
                            }
                            SetupStep::PickProject => {
                                ui.add_space(8.0);
                                let is_unity = crate::unity::is_unity_project_root(
                                    &self.unity.project_path,
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "当前绑定：{}",
                                        self.unity.project_path.display()
                                    ))
                                    .size(12.0)
                                    .monospace()
                                    .color(if is_unity { TEXT } else { DANGER }),
                                );
                                if !is_unity {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(
                                            "这是 agent 任务目录或其它非 Unity 路径，不能用于 pipeline install。请改选工程根。",
                                        )
                                        .size(12.0)
                                        .color(DANGER),
                                    );
                                }
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("选择 Unity 工程根目录…")
                                                    .size(12.5)
                                                    .color(BG)
                                                    .strong(),
                                            )
                                            .fill(UNITY_ACCENT)
                                            .min_size(Vec2::new(0.0, 32.0))
                                            .corner_radius(CornerRadius::same(8)),
                                        )
                                        .clicked()
                                    {
                                        self.pick_unity_project(ui.ctx());
                                    }
                                });
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(
                                        "例：C:\\Users\\…\\设置指南编辑器嵌入式教程（不要选到 Assets\\子目录，也不要用 .bony-worktrees\\task-*）",
                                    )
                                    .size(11.5)
                                    .color(MUTED),
                                );
                            }
                            SetupStep::InstallPipeline
                            | SetupStep::ProbeEditor
                            | SetupStep::RunLoop => {
                                ui.add_space(8.0);
                                if matches!(focus, SetupStep::ProbeEditor) {
                                    ui.label(
                                        RichText::new(
                                            "请先用 Unity 6.0+ 打开同一项目并等待编译完成，再点探测。",
                                        )
                                        .size(12.0)
                                        .color(MUTED),
                                    );
                                    ui.add_space(6.0);
                                }
                                let label = focus.primary_label();
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            RichText::new(label)
                                                .size(13.0)
                                                .color(BG)
                                                .strong(),
                                        )
                                        .fill(UNITY_ACCENT)
                                        .min_size(Vec2::new(160.0, 34.0))
                                        .corner_radius(CornerRadius::same(8)),
                                    )
                                    .clicked()
                                {
                                    self.unity.run_setup_primary();
                                }
                                if busy {
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            RichText::new("执行中，请稍候…")
                                                .size(12.0)
                                                .color(MUTED),
                                        );
                                    });
                                }
                            }
                        }
                    });
            });
    }

    fn unity_pipeline_card(&mut self, ui: &mut egui::Ui) {
        let busy = self.unity.busy || self.unity.is_guiding();
        let installing = self.unity.pipeline_status == PipelineStatus::Installing;
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(
                1.0,
                if self.unity.pipeline_ready_for_commands() {
                    Color32::from_rgb(50, 90, 70)
                } else {
                    BORDER
                },
            ))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Pipeline · command / eval 前提")
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let status_color = match self.unity.pipeline_status {
                            PipelineStatus::Installed => OK,
                            PipelineStatus::Installing
                            | PipelineStatus::PendingImport
                            | PipelineStatus::Checking => UNITY_ACCENT,
                            PipelineStatus::NotInstalled | PipelineStatus::Error => DANGER,
                            PipelineStatus::Unknown => MUTED,
                        };
                        ui.label(
                            RichText::new(self.unity.pipeline_status.label())
                                .size(12.0)
                                .color(status_color),
                        );
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "编辑器要响应 unity command / eval，需先在项目中安装 com.unity.pipeline",
                    )
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(10.0);

                for (label, ok, detail) in self.unity.checklist() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(if ok { "●" } else { "○" })
                                .size(12.0)
                                .color(if ok { OK } else { MUTED }),
                        );
                        ui.label(RichText::new(label).size(12.5).strong().color(TEXT));
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(detail).size(11.5).color(MUTED).monospace(),
                        )
                        .wrap(),
                    );
                    ui.add_space(3.0);
                }

                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if !crate::unity::is_unity_project_root(&self.unity.project_path)
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new("先选 Unity 工程…")
                                        .size(12.5)
                                        .color(BG)
                                        .strong(),
                                )
                                .fill(DANGER)
                                .min_size(Vec2::new(0.0, 32.0))
                                .corner_radius(CornerRadius::same(8)),
                            )
                            .clicked()
                    {
                        self.pick_unity_project(ui.ctx());
                    }
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(
                                RichText::new(if installing {
                                    "安装中…"
                                } else if self.unity.pipeline_status == PipelineStatus::PendingImport {
                                    "等待 Unity 加载"
                                } else {
                                    "安装 Pipeline"
                                })
                                .size(12.5)
                                .color(BG)
                                .strong(),
                            )
                            .fill(UNITY_ACCENT)
                            .min_size(Vec2::new(0.0, 32.0))
                            .corner_radius(CornerRadius::same(8)),
                        )
                        .on_hover_text("在当前项目目录执行：unity pipeline install")
                        .clicked()
                    {
                        self.unity.run_action(UnityAction::InstallPipeline);
                    }
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(RichText::new("刷新列表").size(12.5).color(TEXT))
                                .fill(PANEL_2)
                                .min_size(Vec2::new(0.0, 32.0))
                                .corner_radius(CornerRadius::same(8)),
                        )
                        .on_hover_text("unity pipeline list")
                        .clicked()
                    {
                        self.unity.run_action(UnityAction::ListPipeline);
                    }
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(RichText::new("探测编辑器").size(12.5).color(TEXT))
                                .fill(PANEL_2)
                                .min_size(Vec2::new(0.0, 32.0))
                                .corner_radius(CornerRadius::same(8)),
                        )
                        .on_hover_text("unity command --project-path=…（需编辑器已打开项目）")
                        .clicked()
                    {
                        self.unity.run_action(UnityAction::ProbeEditor);
                    }
                });

                ui.add_space(8.0);
                let link_color = match self.unity.editor_link {
                    EditorLinkStatus::Connected => OK,
                    EditorLinkStatus::Disconnected | EditorLinkStatus::Checking => UNITY_ACCENT,
                    EditorLinkStatus::Unknown => MUTED,
                };
                ui.label(
                    RichText::new(format!(
                        "编辑器：{} · {}",
                        self.unity.editor_link.label(),
                        self.unity.commands_summary
                    ))
                    .size(12.0)
                    .color(link_color),
                );

                if !self.unity.pipeline_detail.trim().is_empty() {
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new(
                        RichText::new("Pipeline 输出").size(11.5).color(MUTED),
                    )
                    .id_salt("unity_pipeline_detail")
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.unity.pipeline_detail).monospace(),
                            )
                            .wrap(),
                        );
                    });
                }

                ui.add_space(8.0);
                Frame::new()
                    .fill(PANEL_2)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("步骤提示")
                                .size(12.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(
                                "1. 用 Unity 6.0 LTS 或更新版本打开同一工程  ·  2. 安装 Pipeline  ·  3. 等 Package Manager 下载并完成脚本编译  ·  4. 探测编辑器  ·  5. 仅当 eval 返回未授权时再执行 unity auth login",
                            )
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
            });
    }

    fn unity_status_card(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("CLI 状态").size(14.0).strong().color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = self.unity.busy;
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(RichText::new("重新检测").size(12.0)),
                            )
                            .clicked()
                        {
                            self.unity.run_action(UnityAction::RefreshDetect);
                        }
                    });
                });
                ui.add_space(8.0);

                let path_text = self
                    .unity
                    .cli_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "未找到 unity 二进制".into());
                ui.label(RichText::new(path_text).size(12.5).color(MUTED).monospace());
                if !self.unity.version_line.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(&self.unity.version_line)
                            .size(12.0)
                            .color(TEXT)
                            .monospace(),
                    );
                }
                if let Some(err) = &self.unity.last_error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(err).size(12.0).color(DANGER));
                }

                ui.add_space(10.0);
                for (label, value) in [
                    ("编辑器", self.unity.editors_summary.as_str()),
                    ("Pipeline", self.unity.pipeline_summary.as_str()),
                    ("已注册命令", self.unity.commands_summary.as_str()),
                ] {
                    ui.label(RichText::new(label).size(12.0).color(MUTED));
                    ui.add(egui::Label::new(RichText::new(value).size(12.5).color(TEXT)).wrap());
                    ui.add_space(6.0);
                }

                if matches!(self.unity.status, CliStatus::Missing | CliStatus::Error) {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("本机未检测到 Unity CLI，可先装 beta 通道：")
                            .size(12.5)
                            .color(MUTED),
                    );
                    ui.add_space(6.0);
                    let hint = UnityState::install_hint();
                    Frame::new()
                        .fill(PANEL_2)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(hint).size(11.5).monospace().color(TEXT));
                                if ui.small_button("复制").clicked() {
                                    ui.ctx().copy_text(hint.to_string());
                                    self.unity.toast = Some("安装命令已复制".into());
                                }
                            });
                        });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("未安装时下方操作为演示模式，可预览 AI 闭环可视化。")
                            .size(12.0)
                            .color(UNITY_ACCENT),
                    );
                }
            });
    }

    fn unity_scene_card(&mut self, ui: &mut egui::Ui) {
        let scene = self.unity.scene.clone();
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("场景快照").size(14.0).strong().color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("重置").clicked() {
                            self.unity.reset_scene();
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(RichText::new(&scene.note).size(12.5).color(MUTED));
                ui.add_space(10.0);

                let (response, painter) = ui
                    .allocate_painter(Vec2::new(ui.available_width(), 150.0), egui::Sense::hover());
                let rect = response.rect;
                painter.rect_filled(rect, CornerRadius::same(10), PANEL_2);
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(10),
                    Stroke::new(1.0, BORDER),
                    egui::StrokeKind::Outside,
                );

                // Ground
                let ground_y = rect.bottom() - 36.0;
                let ground_color = if scene.ground_collider_enabled {
                    Color32::from_rgb(70, 140, 100)
                } else {
                    Color32::from_rgb(90, 70, 70)
                };
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 24.0, ground_y),
                        egui::pos2(rect.right() - 24.0, ground_y + 14.0),
                    ),
                    CornerRadius::same(4),
                    ground_color,
                );
                painter.text(
                    egui::pos2(rect.left() + 32.0, ground_y + 1.0),
                    egui::Align2::LEFT_TOP,
                    if scene.ground_collider_enabled {
                        "GroundCollider ON"
                    } else {
                        "GroundCollider OFF"
                    },
                    egui::FontId::proportional(11.0),
                    TEXT,
                );

                // Player
                let player_size = Vec2::new(22.0, 28.0);
                let cx = rect.center().x;
                let floor = ground_y - 2.0;
                let py = if scene.ground_collider_enabled {
                    floor - player_size.y - scene.player_y.max(0.0) * 8.0
                } else {
                    // Falling below ground
                    floor - player_size.y + (-scene.player_y).clamp(0.0, 4.0) * 18.0
                };
                let player_rect = egui::Rect::from_center_size(
                    egui::pos2(cx, py + player_size.y * 0.5),
                    player_size,
                );
                painter.rect_filled(player_rect, CornerRadius::same(5), UNITY_ACCENT);
                painter.text(
                    egui::pos2(cx, player_rect.top() - 4.0),
                    egui::Align2::CENTER_BOTTOM,
                    "Player",
                    egui::FontId::proportional(11.0),
                    MUTED,
                );

                // Play badge
                let play_label = if scene.is_playing { "PLAY" } else { "EDIT" };
                let play_color = if scene.is_playing { OK } else { MUTED };
                painter.text(
                    egui::pos2(rect.right() - 16.0, rect.top() + 12.0),
                    egui::Align2::RIGHT_TOP,
                    play_label,
                    egui::FontId::proportional(12.0),
                    play_color,
                );

                ui.add_space(8.0);
                ui.label(
                    RichText::new(scene.status_line())
                        .size(12.0)
                        .monospace()
                        .color(TEXT),
                );
                ui.label(
                    RichText::new(format!("last eval → {}", scene.last_eval_result))
                        .size(11.5)
                        .monospace()
                        .color(MUTED),
                );
            });
    }

    fn unity_loop_card(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("AI 反馈闭环").size(14.0).strong().color(TEXT));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("对应 Unity CLI + com.unity.pipeline + command eval")
                        .size(12.0)
                        .color(MUTED),
                );
                ui.add_space(12.0);

                let phases = [LoopPhase::Observe, LoopPhase::Act, LoopPhase::Verify];
                let avail = ui.available_width();
                let gap = 10.0;
                let cell_w = ((avail - gap * 2.0) / 3.0).max(120.0);
                ui.horizontal(|ui| {
                    for (i, phase) in phases.iter().enumerate() {
                        let active = self.unity.loop_phase == *phase;
                        let stroke = if active {
                            Stroke::new(1.5, UNITY_ACCENT)
                        } else {
                            Stroke::new(1.0, BORDER)
                        };
                        let fill = if active { PANEL_2 } else { PANEL };
                        Frame::new()
                            .fill(fill)
                            .stroke(stroke)
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(cell_w - 4.0);
                                ui.label(
                                    RichText::new(format!("0{}", i + 1))
                                        .size(11.0)
                                        .color(if active { UNITY_ACCENT } else { MUTED }),
                                );
                                ui.label(
                                    RichText::new(phase.label()).size(15.0).strong().color(TEXT),
                                );
                                ui.add_space(4.0);
                                ui.label(RichText::new(phase.blurb()).size(11.5).color(MUTED));
                            });
                        if i + 1 < phases.len() {
                            ui.add_space(gap);
                        }
                    }
                });
            });
    }

    fn unity_actions_card(&mut self, ui: &mut egui::Ui) {
        let busy = self.unity.busy || self.unity.is_guiding();
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("可视化操作").size(14.0).strong().color(TEXT));
                ui.add_space(8.0);

                ui.horizontal_wrapped(|ui| {
                    let actions = [
                        UnityAction::ListEditors,
                        UnityAction::ListPipeline,
                        UnityAction::InstallPipeline,
                        UnityAction::ProbeEditor,
                        UnityAction::ListCommands,
                        UnityAction::ObserveCollider,
                        UnityAction::FixCollider,
                        UnityAction::EnterPlayMode,
                        UnityAction::ExitPlayMode,
                    ];
                    for action in actions {
                        let emphasize = matches!(action, UnityAction::InstallPipeline)
                            && !self.unity.pipeline_ready_for_commands();
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(
                                    RichText::new(action.label())
                                        .size(12.5)
                                        .color(if emphasize { BG } else { TEXT }),
                                )
                                .fill(if emphasize { UNITY_ACCENT } else { PANEL_2 })
                                .min_size(Vec2::new(0.0, 30.0))
                                .corner_radius(CornerRadius::same(8)),
                            )
                            .clicked()
                        {
                            self.unity.run_action(action);
                        }
                    }
                });

                ui.add_space(12.0);
                ui.label(RichText::new("unity command eval").size(12.0).color(MUTED));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, expr) in EVAL_PRESETS {
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(RichText::new(*label).size(11.5).color(MUTED))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::new(1.0, BORDER))
                                    .corner_radius(CornerRadius::same(6)),
                            )
                            .clicked()
                        {
                            self.unity.eval_input = (*expr).into();
                        }
                    }
                });
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::multiline(&mut self.unity.eval_input)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("return Application.version;"),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(
                                RichText::new("运行 Eval").size(13.0).color(BG).strong(),
                            )
                            .fill(UNITY_ACCENT)
                            .min_size(Vec2::new(110.0, 32.0))
                            .corner_radius(CornerRadius::same(8)),
                        )
                        .clicked()
                    {
                        self.unity.run_action(UnityAction::Eval);
                    }
                    if busy {
                        ui.spinner();
                        ui.label(RichText::new("执行中…").size(12.0).color(MUTED));
                    }
                });
            });
    }

    fn unity_history_card(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("操作时间线").size(14.0).strong().color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.unity.history.is_empty() && ui.small_button("清空").clicked() {
                            self.unity.clear_history();
                        }
                    });
                });
                ui.add_space(8.0);

                if self.unity.history.is_empty() {
                    ui.label(
                        RichText::new("还没有操作。点「跑完整闭环」或逐步执行观察/修复/验证。")
                            .size(12.5)
                            .color(MUTED),
                    );
                    return;
                }

                let records = self.unity.history.clone();
                for rec in &records {
                    let border = if rec.ok {
                        Stroke::new(1.0, Color32::from_rgb(50, 90, 70))
                    } else {
                        Stroke::new(1.0, Color32::from_rgb(110, 50, 50))
                    };
                    Frame::new()
                        .fill(PANEL_2)
                        .stroke(border)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(rec.phase.label())
                                        .size(11.0)
                                        .color(UNITY_ACCENT),
                                );
                                ui.label(RichText::new(&rec.title).size(13.0).strong().color(TEXT));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!("{} ms", rec.elapsed_ms))
                                                .size(11.0)
                                                .color(MUTED),
                                        );
                                        ui.label(
                                            RichText::new(format_relative(rec.at_unix))
                                                .size(10.5)
                                                .color(MUTED),
                                        );
                                        ui.label(
                                            RichText::new(if rec.ok { "OK" } else { "ERR" })
                                                .size(11.0)
                                                .color(if rec.ok { OK } else { DANGER }),
                                        );
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(&rec.command)
                                    .size(11.0)
                                    .monospace()
                                    .color(MUTED),
                            );
                            ui.add_space(4.0);
                            ui.label(RichText::new(&rec.summary).size(12.5).color(TEXT));
                            if !rec.detail.trim().is_empty()
                                && rec.detail.trim() != rec.summary.trim()
                            {
                                ui.add_space(6.0);
                                egui::CollapsingHeader::new(
                                    RichText::new("详情").size(11.5).color(MUTED),
                                )
                                .id_salt(format!("unity_op_{}", rec.id))
                                .show(ui, |ui| {
                                    ui.monospace(&rec.detail);
                                });
                            }
                        });
                    ui.add_space(8.0);
                }
            });
    }

    fn about_modal(&mut self, ctx: &egui::Context) {
        if !self.model.show_about {
            return;
        }
        let mut open = true;
        egui::Window::new("关于 Bony Build")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.label(RichText::new("Bony Build").size(18.0).strong().color(TEXT));
                ui.add_space(6.0);
                ui.label(
                    RichText::new("原生桌面客户端 · ACP + Grok agent")
                        .size(13.0)
                        .color(MUTED),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("版本 {}", env!("CARGO_PKG_VERSION")))
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(14.0);
                if ui.button("关闭").clicked() {
                    self.model.show_about = false;
                }
            });
        if !open {
            self.model.show_about = false;
        }
    }

    fn unity_composer_chips(&mut self, ui: &mut egui::Ui) {
        let busy = self.unity.busy || self.unity.is_guiding() || self.model.busy;
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(12))
            .stroke(Stroke::new(1.0, UNITY_ACCENT))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Unity CLI")
                            .size(12.0)
                            .strong()
                            .color(UNITY_ACCENT),
                    );
                    ui.label(
                        RichText::new("本地执行 · 也可直接打「探测编辑器」")
                            .size(11.5)
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("收起").clicked() {
                            self.unity_chat_mode = false;
                        }
                        if ui.small_button("设置").clicked() {
                            self.model.main_nav = MainNav::Unity;
                            self.unity.ensure_detecting();
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for cmd in UNITY_CHAT_CHIPS {
                        let clicked = ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(RichText::new(cmd.chip).size(12.0))
                                    .fill(PANEL_2)
                                    .stroke(Stroke::new(1.0, BORDER))
                                    .corner_radius(CornerRadius::same(8)),
                            )
                            .on_hover_text(cmd.slash)
                            .clicked();
                        if clicked {
                            self.dispatch_unity_chat_cmd(cmd, None);
                        }
                    }
                });
            });
    }

    fn floating_composer(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(18))
            .stroke(Stroke::new(
                1.0,
                if self.unity_chat_mode {
                    UNITY_ACCENT
                } else {
                    BORDER
                },
            ))
            .shadow(Shadow {
                offset: [0, 6],
                blur: 24,
                spread: 0,
                color: Color32::from_black_alpha(100),
            })
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let hint = if self.model.needs_login {
                    "请先登录或配置 API Key…"
                } else if self.unity_chat_mode {
                    "Unity：探测编辑器 / 进入 Play · 本地 CLI，无需等 Agent"
                } else if !self.model.connected {
                    "正在连接 agent…"
                } else if self.model.is_viewing_history() {
                    "要求后续变更（将回到当前会话）…"
                } else {
                    "要求后续变更…  Enter 发送 · Shift+Enter 换行 · 点 Unity 控制编辑器"
                };

                let edit = egui::TextEdit::multiline(&mut self.model.draft)
                    .desired_width(f32::INFINITY)
                    .desired_rows(2)
                    .frame(false)
                    .interactive(!self.model.needs_login)
                    .hint_text(RichText::new(hint).color(MUTED));
                let response = ui.add(edit);
                if self.model.focus_composer {
                    response.request_focus();
                    self.model.focus_composer = false;
                }

                let enter_send = response.has_focus()
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Enter)
                            && !i.modifiers.shift
                            && !i.modifiers.ctrl
                            && !i.modifiers.command
                    });
                if enter_send {
                    self.model.draft = self
                        .model
                        .draft
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .to_string();
                    self.send_prompt();
                }

                ui.add_space(8.0);
                if !self.attachments.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for a in &self.attachments {
                            ui.label(
                                RichText::new(format!("📎 {}", a.name))
                                    .size(11.5)
                                    .color(MUTED),
                            );
                        }
                        if ui.small_button("清除").clicked() {
                            self.attachments.clear();
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("＋").size(16.0).color(MUTED))
                        .on_hover_text("添加文件或图片")
                        .clicked()
                    {
                        self.pick_attachments();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_send = (self.model.connected
                            && !self.model.busy
                            && !self.model.needs_login
                            && (!self.model.draft.trim().is_empty()
                                || !self.attachments.is_empty()))
                            || (!self.model.draft.trim().is_empty()
                                && (parse_unity_chat_command(self.model.draft.trim()).is_some()
                                    || wants_unity_help(self.model.draft.trim())));
                        if ui
                            .add_enabled(
                                can_send,
                                egui::Button::new(
                                    RichText::new("发送").size(12.0).color(BG).strong(),
                                )
                                .fill(if can_send { ACCENT } else { PANEL_2 })
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(56.0, 30.0)),
                            )
                            .clicked()
                        {
                            self.send_prompt();
                        }

                        if (self.model.busy || self.stop_armed_force)
                            && ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(if self.stop_armed_force {
                                            "强制停止"
                                        } else {
                                            "停止"
                                        })
                                        .size(12.0)
                                        .color(if self.stop_armed_force { DANGER } else { TEXT }),
                                    )
                                    .fill(PANEL_2)
                                    .stroke(Stroke::new(
                                        1.0,
                                        if self.stop_armed_force {
                                            DANGER
                                        } else {
                                            BORDER
                                        },
                                    ))
                                    .corner_radius(CornerRadius::same(10))
                                    .min_size(Vec2::new(64.0, 28.0)),
                                )
                                .on_hover_text(if self.stop_armed_force {
                                    "结束卡住的 agent 子进程并重连"
                                } else {
                                    "请求停止当前回合；若无效再点一次强制停止"
                                })
                                .clicked()
                        {
                            if self.stop_armed_force {
                                self.send_cmd(UiCommand::ForceStop);
                                self.model.busy = false;
                                self.model.status = "强制停止，正在重连…".into();
                                self.stop_armed_force = false;
                            } else {
                                self.send_cmd(UiCommand::Cancel);
                                // Unlock immediately — cancel used to sit behind a
                                // blocked prompt await and never reach the bridge.
                                self.model.busy = false;
                                self.model.status = "已请求停止（仍卡住再点「强制停止」）".into();
                                self.stop_armed_force = true;
                            }
                        }

                        let u = &self.model.usage.cumulative;
                        let usage_label = format!("Σ {}", format_tokens(u.total_tokens));
                        if soft_chip(ui, &usage_label, true) {
                            self.model.show_usage_detail = true;
                            self.model.show_user_menu = false;
                        }
                        ui.add_space(4.0);

                        let mode_label = self
                            .model
                            .available_modes
                            .iter()
                            .find(|m| m.id == self.model.current_mode_id)
                            .map(|m| m.name.as_str())
                            .unwrap_or("执行模式");
                        if !self.model.available_modes.is_empty()
                            && soft_chip(ui, mode_label, self.model.connected && !self.model.busy)
                        {
                            let next = self
                                .model
                                .available_modes
                                .iter()
                                .find(|m| m.id != self.model.current_mode_id)
                                .map(|m| m.id.clone());
                            if let Some(mode_id) = next {
                                self.send_cmd(UiCommand::SetMode { mode_id });
                            }
                        }
                        ui.add_space(4.0);

                        let model_label = if self.model.current_model_name.is_empty() {
                            "选择模型"
                        } else {
                            self.model.current_model_name.as_str()
                        };
                        if soft_chip(
                            ui,
                            model_label,
                            self.model.connected && !self.model.needs_login,
                        ) {
                            self.model.show_model_picker = true;
                        }

                        ui.add_space(4.0);
                        let unity_on = self.unity_chat_mode;
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Unity")
                                        .size(12.0)
                                        .color(if unity_on { BG } else { UNITY_ACCENT })
                                        .strong(),
                                )
                                .fill(if unity_on { UNITY_ACCENT } else { PANEL_2 })
                                .stroke(Stroke::new(
                                    1.0,
                                    if unity_on { UNITY_ACCENT } else { BORDER },
                                ))
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(56.0, 28.0)),
                            )
                            .on_hover_text(
                                "打开 Unity 对话控制：点芯片或说「探测编辑器」走本地 CLI",
                            )
                            .clicked()
                        {
                            self.unity_chat_mode = !unity_on;
                            if self.unity_chat_mode {
                                self.unity.ensure_detecting();
                            }
                        }
                    });
                });
            });
    }

    fn user_menu_popup(&mut self, ctx: &egui::Context) {
        if !self.model.show_user_menu {
            return;
        }

        let mut open = true;
        egui::Window::new("user_menu")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::LEFT_BOTTOM, [12.0, -56.0])
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(12))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(10, 10))
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    }),
            )
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(220.0);
                ui.horizontal(|ui| {
                    avatar_circle(ui, &self.model.initials());
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(&self.model.display_name)
                            .size(14.0)
                            .strong()
                            .color(TEXT),
                    );
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                if menu_row(ui, "使用统计", true) {
                    self.model.show_usage_detail = true;
                    self.model.show_user_menu = false;
                }
                if menu_row(ui, "设置 · 编辑 config.toml", false) {
                    self.model.show_user_menu = false;
                    if let Err(e) = crate::config_io::open_config_in_editor() {
                        self.model
                            .apply(AgentEvent::Error(format!("无法打开配置: {e}")));
                    }
                }
                if self.model.needs_login {
                    if menu_row(ui, "登录", false) {
                        self.model.show_user_menu = false;
                        self.send_cmd(UiCommand::Login);
                    }
                } else if menu_row(ui, "重新登录", false) {
                    self.model.show_user_menu = false;
                    self.send_cmd(UiCommand::Login);
                }
            });

        if !open {
            self.model.show_user_menu = false;
        }

        // Click-away: close when pointer down outside (simple: Esc).
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.model.show_user_menu = false;
        }
    }

    fn task_error_modal(&mut self, ctx: &egui::Context) {
        let Some(message) = self.task_error.clone() else {
            return;
        };
        egui::Window::new("操作未完成")
            .collapsible(false)
            .resizable(true)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label(RichText::new(message).color(DANGER));
                ui.add_space(12.0);
                if ui.button("关闭").clicked() {
                    self.task_error = None;
                }
            });
    }

    fn git_confirmation_modal(&mut self, ctx: &egui::Context) {
        let Some((stage, path)) = self.pending_git_action.clone() else {
            return;
        };
        egui::Window::new(if stage {
            "确认暂存"
        } else {
            "确认取消暂存"
        })
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("将对 {} 执行显式 Git 写操作。", path.display()));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("取消").clicked() {
                    self.pending_git_action = None;
                }
                if ui.button("确认").clicked() {
                    let result = if stage {
                        GitWorkspaceService::stage(&self.config.cwd, &path)
                    } else {
                        GitWorkspaceService::unstage(&self.config.cwd, &path)
                    };
                    match result {
                        Ok(()) => {
                            self.changes =
                                GitWorkspaceService::changes(&self.config.cwd).unwrap_or_default()
                        }
                        Err(e) => self.task_error = Some(e),
                    }
                    self.pending_git_action = None;
                }
            });
        });
    }

    /// Centered usage sheet: clean single card, tabs, no nested sidebar.
    fn usage_detail_window(&mut self, ctx: &egui::Context) {
        if !self.model.show_usage_detail {
            return;
        }

        let screen = ctx.screen_rect();
        let panel_w = (screen.width() * 0.58).clamp(560.0, 820.0);
        let panel_h = (screen.height() * 0.78).clamp(480.0, 720.0);

        let mut close = false;
        egui::Area::new(egui::Id::new("usage_dim"))
            .fixed_pos(screen.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(170));
                if resp.clicked() {
                    close = true;
                }
            });

        let model_stats = aggregate_model_usage(&self.model.history_turns);
        let turns: Vec<_> = self.model.history_turns.iter().rev().cloned().collect();
        // Prefer history totals so chips match charts (session cumulative can stay 0
        // after "new task" clears local session turns).
        let hist_total: u64 = self
            .model
            .history_turns
            .iter()
            .map(|t| t.usage_delta.total_tokens)
            .sum();
        let hist_in: u64 = self
            .model
            .history_turns
            .iter()
            .map(|t| t.usage_delta.input_tokens)
            .sum();
        let hist_out: u64 = self
            .model
            .history_turns
            .iter()
            .map(|t| t.usage_delta.output_tokens)
            .sum();
        let sess = &self.model.usage.cumulative;
        let chip_total = hist_total.max(sess.total_tokens);
        let chip_in = hist_in.max(sess.input_tokens);
        let chip_out = hist_out.max(sess.output_tokens);
        let sess_turns = self
            .model
            .history_turns
            .len()
            .max(self.model.usage.turns.len());
        let ctx_used = sess.context_used;
        let ctx_size = sess.context_size;
        let mut open = true;
        let tab = self.model.usage_tab;

        egui::Window::new("使用统计")
            .id(egui::Id::new("usage_sheet"))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([panel_w, panel_h])
            .order(egui::Order::Foreground)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(16))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(22, 18))
                    .shadow(Shadow {
                        offset: [0, 16],
                        blur: 48,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    }),
            )
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_size(Vec2::new(panel_w - 8.0, panel_h - 8.0));

                // Header
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("使用统计").size(18.0).strong().color(TEXT));
                        ui.label(
                            RichText::new("折线 / 柱状统计 · 模型与轮次明细")
                                .size(12.5)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("关闭").size(12.5).color(TEXT))
                                    .fill(PANEL_2)
                                    .stroke(Stroke::new(1.0, BORDER))
                                    .corner_radius(CornerRadius::same(8))
                                    .min_size(Vec2::new(64.0, 30.0)),
                            )
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });

                ui.add_space(14.0);

                // Summary chips row
                ui.horizontal(|ui| {
                    stat_chip(ui, "轮次", &sess_turns.to_string());
                    ui.add_space(8.0);
                    stat_chip(ui, "合计", &format_tokens(chip_total));
                    ui.add_space(8.0);
                    stat_chip(ui, "输入", &format_tokens(chip_in));
                    ui.add_space(8.0);
                    stat_chip(ui, "输出", &format_tokens(chip_out));
                    if let (Some(used), Some(size)) = (ctx_used, ctx_size) {
                        ui.add_space(8.0);
                        stat_chip(
                            ui,
                            "上下文",
                            &format!("{}/{}", format_tokens(used), format_tokens(size)),
                        );
                    }
                });

                ui.add_space(14.0);

                // Tabs
                ui.horizontal(|ui| {
                    if segment_tab(ui, "统计图", tab == UsageTab::Charts) {
                        self.model.usage_tab = UsageTab::Charts;
                    }
                    ui.add_space(6.0);
                    if segment_tab(ui, "模型", tab == UsageTab::Models) {
                        self.model.usage_tab = UsageTab::Models;
                    }
                    ui.add_space(6.0);
                    if segment_tab(
                        ui,
                        &format!("轮次 ({})", turns.len()),
                        tab == UsageTab::Turns,
                    ) {
                        self.model.usage_tab = UsageTab::Turns;
                    }
                });

                ui.add_space(10.0);

                let list_h = (panel_h - 200.0).max(180.0);
                egui::ScrollArea::vertical()
                    .id_salt("usage_sheet_scroll")
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.model.usage_tab {
                            UsageTab::Charts => {
                                // Chronological for charts (history_turns is oldest→newest).
                                charts::draw_usage_charts(
                                    ui,
                                    &self.model.history_turns,
                                    &model_stats,
                                );
                            }
                            UsageTab::Models => {
                                if model_stats.is_empty() {
                                    empty_hint(ui, "还没有模型用量。发送一条消息后会出现在这里。");
                                } else {
                                    for m in &model_stats {
                                        let pct = if chip_total > 0 {
                                            (m.total_tokens as f32 / chip_total as f32)
                                                .clamp(0.0, 1.0)
                                        } else if model_stats.len() == 1 {
                                            1.0
                                        } else {
                                            0.0
                                        };
                                        Frame::new()
                                            .fill(BG)
                                            .corner_radius(CornerRadius::same(12))
                                            .stroke(Stroke::new(1.0, BORDER))
                                            .inner_margin(Margin::symmetric(14, 12))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.horizontal(|ui| {
                                                    ui.vertical(|ui| {
                                                        ui.label(
                                                            RichText::new(&m.model_name)
                                                                .size(14.5)
                                                                .strong()
                                                                .color(TEXT),
                                                        );
                                                        if !m.model_id.is_empty()
                                                            && m.model_id != m.model_name
                                                        {
                                                            ui.label(
                                                                RichText::new(&m.model_id)
                                                                    .size(11.5)
                                                                    .monospace()
                                                                    .color(MUTED),
                                                            );
                                                        }
                                                    });
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "Σ {}",
                                                                    format_tokens(m.total_tokens)
                                                                ))
                                                                .size(14.0)
                                                                .strong()
                                                                .color(ACCENT_BAR),
                                                            );
                                                        },
                                                    );
                                                });
                                                ui.add_space(8.0);
                                                // Progress bar for share of session tokens
                                                let bar_w = ui.available_width();
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(bar_w, 4.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    CornerRadius::same(2),
                                                    PANEL_2,
                                                );
                                                if pct > 0.0 {
                                                    let mut fill = rect;
                                                    fill.set_width((rect.width() * pct).max(4.0));
                                                    ui.painter().rect_filled(
                                                        fill,
                                                        CornerRadius::same(2),
                                                        ACCENT_BAR,
                                                    );
                                                }
                                                ui.add_space(8.0);
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} 轮  ·  in {}  ·  out {}",
                                                        m.turn_count,
                                                        format_tokens(m.input_tokens),
                                                        format_tokens(m.output_tokens),
                                                    ))
                                                    .size(12.5)
                                                    .color(MUTED),
                                                );
                                            });
                                        ui.add_space(10.0);
                                    }
                                }
                            }
                            UsageTab::Turns => {
                                if turns.is_empty() {
                                    empty_hint(ui, "还没有对话轮次记录。");
                                } else {
                                    for turn in &turns {
                                        let expanded = self.model.is_history_expanded(&turn.id);
                                        let model = if turn.model_name.is_empty() {
                                            turn.model_id.as_str()
                                        } else {
                                            turn.model_name.as_str()
                                        };
                                        let chevron = if expanded { "▾" } else { "▸" };
                                        let resp = Frame::new()
                                            .fill(BG)
                                            .corner_radius(CornerRadius::same(12))
                                            .stroke(Stroke::new(
                                                1.0,
                                                if expanded { ACCENT_BAR } else { BORDER },
                                            ))
                                            .inner_margin(Margin::symmetric(14, 12))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(chevron)
                                                            .size(13.0)
                                                            .color(MUTED),
                                                    );
                                                    ui.label(
                                                        RichText::new(model)
                                                            .size(13.5)
                                                            .strong()
                                                            .color(TEXT),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "Δ {}",
                                                                    format_tokens(
                                                                        turn.usage_delta
                                                                            .total_tokens
                                                                    )
                                                                ))
                                                                .size(13.0)
                                                                .color(ACCENT_BAR),
                                                            );
                                                        },
                                                    );
                                                });
                                                ui.add_space(4.0);
                                                ui.label(
                                                    RichText::new(truncate_chars(
                                                        &turn.user_text,
                                                        90,
                                                    ))
                                                    .size(13.0)
                                                    .color(MUTED),
                                                );
                                                if expanded {
                                                    ui.add_space(10.0);
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "in {} · out {} · 停止 {}",
                                                            format_tokens(
                                                                turn.usage_delta.input_tokens
                                                            ),
                                                            format_tokens(
                                                                turn.usage_delta.output_tokens
                                                            ),
                                                            turn.stop_reason,
                                                        ))
                                                        .size(12.0)
                                                        .color(MUTED),
                                                    );
                                                    if !turn.tool_titles.is_empty() {
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "工具 · {}",
                                                                turn.tool_titles.join(" · ")
                                                            ))
                                                            .size(12.0)
                                                            .color(MUTED),
                                                        );
                                                    }
                                                    ui.add_space(6.0);
                                                    ui.label(
                                                        RichText::new("助手回复")
                                                            .size(11.5)
                                                            .strong()
                                                            .color(OK),
                                                    );
                                                    ui.label(
                                                        RichText::new(truncate_chars(
                                                            &turn.assistant_text,
                                                            500,
                                                        ))
                                                        .size(12.5)
                                                        .color(TEXT),
                                                    );
                                                } else {
                                                    ui.label(
                                                        RichText::new("点击展开详情")
                                                            .size(11.5)
                                                            .color(MUTED),
                                                    );
                                                }
                                            })
                                            .response
                                            .interact(egui::Sense::click());
                                        if resp.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                        if resp.clicked() {
                                            self.model.toggle_history_expanded(&turn.id);
                                        }
                                        ui.add_space(10.0);
                                    }
                                }
                            }
                        }
                    });
            });

        if !open || close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.model.show_usage_detail = false;
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("今天想做什么？")
                    .size(28.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("写代码用 Agent；控编辑器用下方 Unity 指令（本地 CLI）。")
                    .size(14.0)
                    .color(MUTED),
            );
        });

        if self.model.needs_login {
            // Login lives in the sidebar bottom-left — keep the hero clean.
            return;
        }

        ui.add_space(24.0);
        self.unity_chat_entry_card(ui);

        ui.add_space(20.0);
        ui.label(RichText::new("快速开始 · 代码").size(12.5).color(MUTED));
        ui.add_space(10.0);

        let starters: Vec<&str> = STARTERS.to_vec();
        for starter in starters {
            let enabled = self.model.connected && !self.model.busy;
            let resp = Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(12))
                .stroke(Stroke::new(1.0, BORDER))
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let color = if enabled { TEXT } else { MUTED };
                    ui.label(RichText::new(starter).size(14.0).color(color));
                })
                .response
                .interact(egui::Sense::click());

            if enabled && resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if enabled && resp.clicked() {
                self.send_starter(starter);
            }
            ui.add_space(8.0);
        }
    }

    /// Discoverable entry for conversation → Unity CLI control.
    fn unity_chat_entry_card(&mut self, ui: &mut egui::Ui) {
        let busy = self.unity.busy || self.unity.is_guiding() || self.model.busy;
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(14))
            .stroke(Stroke::new(1.5, UNITY_ACCENT))
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Unity 对话控制")
                            .size(15.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(RichText::new(self.unity.status.label()).size(12.0).color(
                        match self.unity.status {
                            CliStatus::Ready => OK,
                            CliStatus::Missing | CliStatus::Error => DANGER,
                            _ => MUTED,
                        },
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("设置页")
                            .on_hover_text("打开引导：选工程 / 装 Pipeline")
                            .clicked()
                        {
                            self.model.main_nav = MainNav::Unity;
                            self.unity.ensure_detecting();
                        }
                        if ui
                            .small_button("全部指令")
                            .on_hover_text("在聊天里列出 /unity 指令")
                            .clicked()
                        {
                            self.unity_chat_mode = true;
                            self.model.push_local_assistant(unity_chat_help_text());
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("点芯片立刻执行本地 Unity CLI，不会让 Agent 去跑终端。")
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for cmd in UNITY_CHAT_CHIPS {
                        let resp = ui.add_enabled(
                            !busy,
                            egui::Button::new(
                                RichText::new(cmd.chip).size(12.5).color(BG).strong(),
                            )
                            .fill(UNITY_ACCENT)
                            .corner_radius(CornerRadius::same(8))
                            .min_size(Vec2::new(0.0, 28.0)),
                        );
                        if resp.on_hover_text(cmd.slash).clicked() {
                            self.dispatch_unity_chat_cmd(cmd, None);
                        }
                    }
                });
            });
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let len = self.model.timeline.len();
        for idx in 0..len {
            match self.model.timeline.get(idx).cloned() {
                Some(TimelineItem::Message(msg)) => self.message_block(ui, &msg),
                Some(TimelineItem::Tool(card)) => {
                    let mut open = card.open;
                    self.tool_block(ui, &card, &mut open);
                    if let Some(TimelineItem::Tool(c)) = self.model.timeline.get_mut(idx) {
                        c.open = open;
                    }
                }
                None => {}
            }
            ui.add_space(16.0);
        }
    }

    fn message_block(&self, ui: &mut egui::Ui, msg: &crate::model::ChatMessage) {
        match msg.role {
            Role::User => {
                let display_text = if msg.text.contains("只读分析")
                    && msg.text.contains("Unity 面板状态")
                {
                    "分析当前 Unity 状态"
                } else {
                    msg.text.as_str()
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    Frame::new()
                        .fill(USER_BG)
                        .corner_radius(CornerRadius {
                            nw: 14,
                            ne: 14,
                            sw: 4,
                            se: 14,
                        })
                        .inner_margin(Margin::symmetric(14, 11))
                        .show(ui, |ui| {
                            ui.set_max_width(MAX_CHAT_W * 0.72);
                            ui.label(RichText::new(display_text).size(14.5).color(TEXT));
                        });
                });
            }
            Role::Assistant => {
                Frame::new()
                    .fill(ASSIST_BG)
                    .corner_radius(CornerRadius::same(12))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let (bar, _) =
                                ui.allocate_exact_size(Vec2::new(3.0, 28.0), egui::Sense::hover());
                            ui.painter()
                                .rect_filled(bar, CornerRadius::same(2), ACCENT_BAR);
                            ui.add_space(10.0);
                            // Pin an explicit content width so wrapped inline code
                            // never collapses to a 1-glyph column.
                            let content_w = ui.available_width().max(40.0);
                            ui.allocate_ui_with_layout(
                                Vec2::new(content_w, 0.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    ui.set_width(content_w);
                                    markdown::render(ui, &msg.text, TEXT);
                                },
                            );
                        });
                    });
                if let Some(u) = &msg.turn_usage {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "本轮 · Σ {} · in {} · out {}{}",
                            format_tokens(u.total_tokens),
                            format_tokens(u.input_tokens),
                            format_tokens(u.output_tokens),
                            if u.thought_tokens > 0 {
                                format!(" · think {}", format_tokens(u.thought_tokens))
                            } else {
                                String::new()
                            }
                        ))
                        .size(11.5)
                        .color(MUTED),
                    );
                }
            }
            Role::System => {
                Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(10))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(90, 50, 50)))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new("系统").size(11.5).color(DANGER));
                        ui.add_space(4.0);
                        ui.label(RichText::new(&msg.text).size(13.0).color(MUTED));
                    });
            }
        }
    }

    fn tool_block(&self, ui: &mut egui::Ui, card: &crate::model::ToolCard, open: &mut bool) {
        let status_color = if card.status.contains("Completed") || card.status.contains("completed")
        {
            OK
        } else if card.status.contains("Failed") || card.status.contains("Error") {
            DANGER
        } else {
            MUTED
        };

        Frame::new()
            .fill(TOOL_BG)
            .corner_radius(CornerRadius::same(10))
            .stroke(Stroke::new(1.0, BORDER))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let chevron = if *open { "▾" } else { "▸" };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{chevron}  工具 · {}", card.title))
                                    .size(12.5)
                                    .color(TEXT),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                        )
                        .clicked()
                    {
                        *open = !*open;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(short_status(&card.status))
                                .size(11.5)
                                .color(status_color),
                        );
                    });
                });
                if *open && !card.detail.is_empty() {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(&card.detail)
                                .size(12.0)
                                .monospace()
                                .color(MUTED),
                        )
                        .wrap(),
                    );
                }
            });
    }

    fn model_picker_modal(&mut self, ctx: &egui::Context) {
        if !self.model.show_model_picker {
            return;
        }

        let mut open = true;
        egui::Window::new("选择模型")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(16))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::same(16))
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.set_max_height(480.0);
                ui.label(
                    RichText::new("切换当前会话使用的模型")
                        .size(13.0)
                        .color(MUTED),
                );
                ui.add_space(10.0);

                let models = self.model.available_models.clone();
                let current = self.model.current_model_id.clone();
                let busy = self.model.busy;

                if models.is_empty() {
                    ui.label(
                        RichText::new("暂无可用模型。可在 config.toml 的 [models] 里配置。")
                            .size(13.0)
                            .color(MUTED),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for m in &models {
                                let selected = m.id == current;
                                let fill = if selected { SELECTED } else { PANEL_2 };
                                let stroke = if selected {
                                    Stroke::new(1.0, ACCENT_BAR)
                                } else {
                                    Stroke::new(1.0, BORDER)
                                };
                                let resp = Frame::new()
                                    .fill(fill)
                                    .corner_radius(CornerRadius::same(10))
                                    .stroke(stroke)
                                    .inner_margin(Margin::symmetric(12, 10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(&m.name)
                                                    .size(14.0)
                                                    .strong()
                                                    .color(TEXT),
                                            );
                                            if selected {
                                                ui.label(
                                                    RichText::new("当前").size(11.5).color(OK),
                                                );
                                            }
                                        });
                                        ui.label(
                                            RichText::new(&m.id)
                                                .size(11.5)
                                                .monospace()
                                                .color(MUTED),
                                        );
                                        if !m.description.is_empty() {
                                            ui.label(
                                                RichText::new(&m.description)
                                                    .size(12.0)
                                                    .color(MUTED),
                                            );
                                        }
                                    })
                                    .response
                                    .interact(egui::Sense::click());

                                if !busy && !selected && resp.clicked() {
                                    self.send_cmd(UiCommand::SetModel {
                                        model_id: m.id.clone(),
                                    });
                                    self.model.status = format!("切换模型 {}…", m.name);
                                }
                                if resp.hovered() && !busy && !selected {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                ui.add_space(8.0);
                            }
                        });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("编辑 config.toml").color(TEXT))
                                .fill(PANEL_2)
                                .stroke(Stroke::new(1.0, BORDER))
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(140.0, 32.0)),
                        )
                        .clicked()
                    {
                        if let Err(e) = crate::config_io::open_config_in_editor() {
                            self.model
                                .apply(AgentEvent::Error(format!("无法打开配置: {e}")));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("关闭").color(TEXT))
                                    .fill(PANEL_2)
                                    .stroke(Stroke::new(1.0, BORDER))
                                    .corner_radius(CornerRadius::same(10))
                                    .min_size(Vec2::new(72.0, 32.0)),
                            )
                            .clicked()
                        {
                            self.model.show_model_picker = false;
                        }
                    });
                });
            });

        if !open {
            self.model.show_model_picker = false;
        }
    }

    fn unity_permission_modal(&mut self, ctx: &egui::Context) {
        let Some(approval) = self.pending_unity_approval.clone() else {
            return;
        };
        egui::Window::new("Unity 操作需要权限")
            .collapsible(false)
            .resizable(true)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                ui.label(RichText::new(&approval.summary).size(16.0).strong());
                ui.add_space(8.0);
                if approval.risks.is_empty() {
                    ui.label("该计划会修改 Unity 编辑器、场景或项目资源。");
                } else {
                    ui.colored_label(
                        DANGER,
                        format!("计划请求高风险能力：{}", approval.risks.join("、")),
                    );
                    ui.label("高风险能力即使在完全控制模式下也会逐次询问。");
                }
                ui.add_space(10.0);
                egui::CollapsingHeader::new("查看生成的 Unity C#")
                    .show(ui, |ui| ui.monospace(&approval.csharp));
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("允许一次").clicked() {
                        self.pending_unity_approval = None;
                        self.execute_unity_plan(approval.summary.clone(), approval.csharp.clone());
                    }
                    if approval.risks.is_empty() && ui.button("当前任务完全控制").clicked()
                    {
                        if let Some(id) = self.active_task_id.clone()
                            && let Some(task) = self.tasks.iter_mut().find(|task| task.id == id)
                        {
                            task.permission_mode = PermissionMode::FullControl;
                            task.updated_at = unix_time();
                            if let Some(repo) = &self.task_repo {
                                let _ = repo.save(task);
                            }
                        }
                        self.pending_unity_approval = None;
                        self.execute_unity_plan(approval.summary.clone(), approval.csharp.clone());
                    }
                    if ui.button("拒绝").clicked() {
                        self.pending_unity_approval = None;
                        self.model.replace_latest_assistant(format!(
                            "已拒绝 Unity 计划：{}。没有执行任何操作。",
                            approval.summary
                        ));
                        self.model.status = "Unity 操作已拒绝".into();
                    }
                });
            });
    }

    fn permission_modal(&mut self, ctx: &egui::Context) {
        let Some(perm) = self.model.pending_permission.clone() else {
            return;
        };

        egui::Area::new(egui::Id::new("perm_dim"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            });

        egui::Window::new("需要批准")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(16))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::same(18))
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label(RichText::new(&perm.title).size(16.0).strong().color(TEXT));
                ui.add_space(6.0);
                ui.label(
                    RichText::new("助手想执行需要你确认的工具。")
                        .size(13.0)
                        .color(MUTED),
                );
                ui.add_space(12.0);
                for opt in &perm.options {
                    ui.label(
                        RichText::new(format!("· {}", opt.name))
                            .size(12.5)
                            .color(MUTED),
                    );
                }
                ui.add_space(16.0);
                ui.horizontal_wrapped(|ui| {
                    for opt in &perm.options {
                        let allow = opt.kind.contains("Allow");
                        if ui
                            .add(
                                egui::Button::new(RichText::new(&opt.name).color(if allow {
                                    BG
                                } else {
                                    TEXT
                                }))
                                .fill(if allow { ACCENT } else { PANEL_2 })
                                .stroke(Stroke::new(1.0, BORDER))
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(100.0, 34.0)),
                            )
                            .clicked()
                        {
                            self.model.pending_permission = None;
                            self.send_cmd(UiCommand::PermissionResponse {
                                option_id: Some(opt.id.clone()),
                            });
                            self.model.status = if allow { "Working…" } else { "Ready" }.into();
                        }
                    }
                    if ui.button("取消").clicked() {
                        self.model.pending_permission = None;
                        self.send_cmd(UiCommand::PermissionResponse { option_id: None });
                    }
                });
            });
    }
}

/// Horizontally center a fixed-width chat column in the main pane.
fn centered_column(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let avail = ui.available_width();
    let width = if avail > MAX_CHAT_W + 48.0 {
        MAX_CHAT_W
    } else {
        (avail - 32.0).clamp(240.0, MAX_CHAT_W)
    };
    let pad = ((avail - width) * 0.5).max(0.0);

    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.allocate_ui_with_layout(
            Vec2::new(width, ui.available_height()),
            egui::Layout::top_down(egui::Align::Min).with_cross_justify(true),
            |ui| {
                ui.set_width(width);
                ui.set_max_width(width);
                add(ui);
            },
        );
    });
}

fn stat_chip(ui: &mut egui::Ui, label: &str, value: &str) {
    Frame::new()
        .fill(BG)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(72.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(11.0).color(MUTED));
                ui.add_space(2.0);
                ui.label(RichText::new(value).size(15.0).strong().color(TEXT));
            });
        });
}

fn segment_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let fill = if selected {
        SELECTED
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0, ACCENT_BAR)
    } else {
        Stroke::new(1.0, BORDER)
    };
    let resp = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .stroke(stroke)
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .size(12.5)
                    .color(if selected { TEXT } else { MUTED }),
            );
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn empty_hint(ui: &mut egui::Ui, text: &str) {
    Frame::new()
        .fill(BG)
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::symmetric(16, 20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(text).size(13.5).color(MUTED));
            });
        });
}

#[derive(Clone, Copy)]
enum NavDir {
    Back,
    Forward,
}

#[derive(Clone, Copy)]
enum PanelSide {
    Left,
    Right,
}

fn panel_toggle_btn(ui: &mut egui::Ui, side: PanelSide, tip: &str, active: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
    let resp = resp.on_hover_text(tip);
    if resp.hovered() || active {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), SELECTED);
    }
    let color = if active || resp.hovered() {
        TEXT
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.3, color);
    let outer = egui::Rect::from_center_size(rect.center(), Vec2::new(14.0, 11.0));
    ui.painter().rect_stroke(
        outer,
        CornerRadius::same(1),
        stroke,
        egui::StrokeKind::Outside,
    );
    match side {
        PanelSide::Left => {
            let x = outer.left() + 4.5;
            ui.painter().line_segment(
                [
                    egui::pos2(x, outer.top() + 1.0),
                    egui::pos2(x, outer.bottom() - 1.0),
                ],
                stroke,
            );
            let pane = egui::Rect::from_min_max(
                egui::pos2(outer.left() + 1.0, outer.top() + 1.0),
                egui::pos2(x, outer.bottom() - 1.0),
            );
            ui.painter()
                .rect_filled(pane, CornerRadius::ZERO, color.linear_multiply(0.35));
        }
        PanelSide::Right => {
            let x = outer.right() - 4.5;
            ui.painter().line_segment(
                [
                    egui::pos2(x, outer.top() + 1.0),
                    egui::pos2(x, outer.bottom() - 1.0),
                ],
                stroke,
            );
            let pane = egui::Rect::from_min_max(
                egui::pos2(x, outer.top() + 1.0),
                egui::pos2(outer.right() - 1.0, outer.bottom() - 1.0),
            );
            ui.painter()
                .rect_filled(pane, CornerRadius::ZERO, color.linear_multiply(0.35));
        }
    }
    resp.clicked()
}

fn nav_chevron_btn(ui: &mut egui::Ui, dir: NavDir, tip: &str, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), egui::Sense::click());
    let resp = resp.on_hover_text(tip);
    // Soft rounded plate on hover (Codex-style).
    if resp.hovered() {
        ui.painter().rect_filled(
            rect.shrink(1.0),
            CornerRadius::same(7),
            Color32::from_rgb(48, 48, 54),
        );
    }

    let color = if !enabled {
        Color32::from_rgb(88, 88, 96)
    } else if resp.hovered() {
        Color32::from_rgb(230, 230, 234)
    } else {
        Color32::from_rgb(168, 168, 176)
    };
    let stroke = Stroke::new(1.6, color);
    let c = rect.center();

    // Shaft + arrowhead (← / →), not a bare chevron.
    let half_len = 6.5;
    let head = 4.2;
    match dir {
        NavDir::Back => {
            let tip = egui::pos2(c.x - half_len, c.y);
            let tail = egui::pos2(c.x + half_len, c.y);
            ui.painter().line_segment([tip, tail], stroke);
            ui.painter()
                .line_segment([egui::pos2(tip.x + head, tip.y - head), tip], stroke);
            ui.painter()
                .line_segment([egui::pos2(tip.x + head, tip.y + head), tip], stroke);
        }
        NavDir::Forward => {
            let tip = egui::pos2(c.x + half_len, c.y);
            let tail = egui::pos2(c.x - half_len, c.y);
            ui.painter().line_segment([tail, tip], stroke);
            ui.painter()
                .line_segment([egui::pos2(tip.x - head, tip.y - head), tip], stroke);
            ui.painter()
                .line_segment([egui::pos2(tip.x - head, tip.y + head), tip], stroke);
        }
    }
    enabled && resp.clicked()
}

fn search_icon_btn(ui: &mut egui::Ui, active: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
    let resp = resp.on_hover_text("搜索任务");
    if resp.hovered() || active {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), SELECTED);
    }
    let color = if active || resp.hovered() {
        TEXT
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.4, color);
    let c = egui::pos2(rect.center().x - 1.2, rect.center().y - 1.2);
    let r = 5.2;
    ui.painter().circle_stroke(c, r, stroke);
    let handle_start = egui::pos2(c.x + r * 0.72, c.y + r * 0.72);
    let handle_end = egui::pos2(rect.center().x + 6.2, rect.center().y + 6.2);
    ui.painter()
        .line_segment([handle_start, handle_end], stroke);
    resp.clicked()
}

#[derive(Clone, Copy)]
enum WinChrome {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Vector window controls — glyphs often fail with CJK UI fonts.
fn win_chrome_btn(ui: &mut egui::Ui, kind: WinChrome) -> bool {
    let danger = matches!(kind, WinChrome::Close);
    let tip = match kind {
        WinChrome::Minimize => "最小化",
        WinChrome::Maximize => "最大化",
        WinChrome::Restore => "还原",
        WinChrome::Close => "关闭",
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(46.0, TITLE_BAR_H), egui::Sense::click());
    let resp = resp.on_hover_text(tip);

    let hover_bg = if danger {
        Color32::from_rgb(196, 64, 64)
    } else {
        Color32::from_rgb(52, 52, 58)
    };
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, hover_bg);
    }

    let icon = if resp.hovered() && danger {
        TEXT
    } else if resp.hovered() {
        TEXT
    } else {
        Color32::from_rgb(200, 200, 206)
    };
    let stroke = Stroke::new(1.35, icon);
    let c = rect.center();

    match kind {
        WinChrome::Minimize => {
            let half = 5.5;
            ui.painter().line_segment(
                [egui::pos2(c.x - half, c.y), egui::pos2(c.x + half, c.y)],
                stroke,
            );
        }
        WinChrome::Maximize => {
            let half = 5.0;
            let r = egui::Rect::from_center_size(c, Vec2::splat(half * 2.0));
            ui.painter()
                .rect_stroke(r, CornerRadius::ZERO, stroke, egui::StrokeKind::Outside);
        }
        WinChrome::Restore => {
            let s = 4.2;
            let back = egui::Rect::from_min_size(
                egui::pos2(c.x - s + 1.5, c.y - s - 1.0),
                Vec2::splat(s * 1.7),
            );
            let front = egui::Rect::from_min_size(
                egui::pos2(c.x - s - 1.0, c.y - s + 1.5),
                Vec2::splat(s * 1.7),
            );
            ui.painter()
                .rect_stroke(back, CornerRadius::ZERO, stroke, egui::StrokeKind::Outside);
            ui.painter().rect_filled(front, CornerRadius::ZERO, SIDEBAR);
            ui.painter()
                .rect_stroke(front, CornerRadius::ZERO, stroke, egui::StrokeKind::Outside);
        }
        WinChrome::Close => {
            let half = 5.0;
            ui.painter().line_segment(
                [
                    egui::pos2(c.x - half, c.y - half),
                    egui::pos2(c.x + half, c.y + half),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(c.x + half, c.y - half),
                    egui::pos2(c.x - half, c.y + half),
                ],
                stroke,
            );
        }
    }

    resp.clicked()
}

fn nav_item(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let fill = if selected {
        SELECTED
    } else {
        Color32::TRANSPARENT
    };
    let resp = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(label)
                    .size(13.5)
                    .color(if selected { TEXT } else { MUTED }),
            );
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn soft_chip(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let color = if enabled { TEXT } else { MUTED };
    let resp = Frame::new()
        .fill(PANEL_2)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 5))
        .stroke(Stroke::new(1.0, BORDER))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        })
        .response
        .interact(egui::Sense::click());
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

fn menu_row(ui: &mut egui::Ui, label: &str, with_chevron: bool) -> bool {
    let resp = Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(13.5).color(TEXT));
                if with_chevron {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(">").size(13.0).color(MUTED));
                    });
                }
            });
        })
        .response
        .interact(egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn avatar_circle(ui: &mut egui::Ui, initials: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 14.0, AVATAR);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initials,
        egui::FontId::proportional(11.0),
        TEXT,
    );
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn short_status(status: &str) -> &str {
    if status.contains("Completed") || status.contains("completed") {
        "完成"
    } else if status.contains("InProgress") || status.contains("started") {
        "运行中"
    } else if status.contains("Failed") || status.contains("Error") {
        "失败"
    } else {
        status
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = PANEL_2;
    visuals.widgets.inactive.bg_fill = PANEL;
    visuals.widgets.hovered.bg_fill = PANEL_2;
    visuals.widgets.active.bg_fill = PANEL_2;
    visuals.selection.bg_fill = Color32::from_rgb(70, 70, 82);
    visuals.override_text_color = Some(TEXT);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(40, 42, 50);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.handle_min_length = 32.0;
    ctx.set_style(style);
}
