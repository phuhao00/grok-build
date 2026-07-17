//! Codex-style shell: left task sidebar, main chat, floating composer.

use std::sync::mpsc;

use eframe::egui::{self, Align2, Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Vec2};
use tokio::sync::mpsc as tokio_mpsc;

use crate::agent_bridge::{self, BridgeConfig};
use crate::events::{AgentEvent, UiCommand};
use crate::markdown;
use crate::model::{AppModel, Role, TimelineItem, UsageTab};
use crate::usage::{aggregate_model_usage, format_tokens};

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
const MAX_CHAT_W: f32 = 860.0;
const SIDEBAR_W: f32 = 248.0;

const STARTERS: &[&str] = &[
    "解释这个代码库的结构",
    "找出最近改动里可能的 bug",
    "给主 agent 循环补测试",
    "总结认证是怎么工作的",
];

pub struct BonyBuildApp {
    model: AppModel,
    event_rx: mpsc::Receiver<AgentEvent>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    cmd_tx: Option<tokio_mpsc::UnboundedSender<UiCommand>>,
    started: bool,
    config: BridgeConfig,
}

impl BonyBuildApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: BridgeConfig) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        configure_style(&cc.egui_ctx);
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            model: AppModel::new(),
            event_rx,
            event_tx: Some(event_tx),
            cmd_tx: None,
            started: false,
            config,
        }
    }

    fn ensure_started(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(event_tx) = self.event_tx.take() {
            let cmd_tx = agent_bridge::spawn_bridge(self.config.clone(), ctx.clone(), event_tx);
            self.cmd_tx = Some(cmd_tx);
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            self.model.apply(ev);
        }
    }

    fn send_cmd(&self, cmd: UiCommand) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    fn send_prompt(&mut self) {
        let text = self.model.draft.trim().to_string();
        if text.is_empty() || self.model.busy || self.model.needs_login || !self.model.connected {
            return;
        }
        self.model.draft.clear();
        self.model.push_user(text.clone());
        self.send_cmd(UiCommand::Prompt(text));
    }

    fn send_starter(&mut self, text: &str) {
        if self.model.busy || !self.model.connected || self.model.needs_login {
            return;
        }
        self.model.draft.clear();
        self.model.push_user(text.to_string());
        self.send_cmd(UiCommand::Prompt(text.to_string()));
    }
}

impl eframe::App for BonyBuildApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_started(ctx);
        self.drain_events();

        if self.model.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }

        egui::SidePanel::left("codex_sidebar")
            .exact_width(SIDEBAR_W)
            .resizable(false)
            .frame(
                Frame::NONE
                    .fill(SIDEBAR)
                    .inner_margin(Margin::symmetric(12, 14))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ctx, |ui| {
                self.sidebar(ui);
            });

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(0, 0)))
            .show(ctx, |ui| {
                // Header (same centered column as chat)
                Frame::NONE
                    .fill(BG)
                    .inner_margin(Margin::symmetric(0, 14))
                    .stroke(Stroke::new(1.0, BORDER))
                    .show(ui, |ui| {
                        centered_column(ui, |ui| {
                            self.main_header(ui);
                        });
                    });

                // Chat + floating composer stacked in a centered column.
                let avail = ui.available_height();
                let composer_h = 140.0;
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
                                    ui.label(
                                        RichText::new("处理中…").size(12.5).color(MUTED),
                                    );
                                });
                            }
                            ui.add_space(20.0);
                        });
                    });

                ui.add_space(8.0);
                centered_column(ui, |ui| {
                    self.floating_composer(ui);
                });
                ui.add_space(12.0);
            });

        self.user_menu_popup(ctx);
        self.usage_detail_window(ctx);
        self.permission_modal(ctx);
        self.model_picker_modal(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send_cmd(UiCommand::Shutdown);
    }
}

impl BonyBuildApp {
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Bony Build")
                    .size(16.0)
                    .strong()
                    .color(TEXT),
            );
        });
        ui.add_space(14.0);

        if nav_item(ui, "新建任务", false) {
            self.model.new_task();
        }
        ui.add_space(2.0);
        if nav_item(ui, "聊天", !self.model.is_viewing_history()) {
            self.model.return_to_live();
        }

        ui.add_space(16.0);
        ui.label(RichText::new("任务").size(12.0).color(MUTED));
        ui.add_space(6.0);

        let tasks = self.model.tasks();
        let viewing = self.model.viewing_session_id.clone();
        let live_id = self.model.session_id.clone();

        egui::ScrollArea::vertical()
            .id_salt("task_list")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if tasks.is_empty() {
                    ui.label(
                        RichText::new("还没有任务记录")
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                for task in &tasks {
                    let selected = viewing
                        .as_ref()
                        .map(|v| v == &task.session_id)
                        .unwrap_or_else(|| {
                            viewing.is_none()
                                && live_id.as_ref().is_some_and(|id| id == &task.session_id)
                        });
                    let fill = if selected { SELECTED } else { Color32::TRANSPARENT };
                    let resp = Frame::new()
                        .fill(fill)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(&task.title)
                                    .size(13.0)
                                    .color(if selected { TEXT } else { MUTED }),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} 轮 · Σ {}",
                                    task.turn_count,
                                    format_tokens(task.total_tokens)
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
                    if resp.clicked() {
                        if live_id.as_ref() == Some(&task.session_id) && viewing.is_none() {
                            // already live for this session
                        } else if live_id.as_ref() == Some(&task.session_id) {
                            self.model.return_to_live();
                        } else {
                            self.model.load_task_view(&task.session_id);
                        }
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

    fn main_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&self.model.task_title)
                    .size(16.0)
                    .strong()
                    .color(TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, color) = if self.model.needs_login {
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
                ui.label(RichText::new(label).size(12.5).color(color));
                if self.model.busy {
                    ui.spinner();
                }
                let folder = self
                    .model
                    .cwd
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "workspace".into());
                ui.label(RichText::new(folder).size(12.0).color(MUTED));
            });
        });
    }

    fn floating_composer(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(18))
            .stroke(Stroke::new(1.0, BORDER))
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
                } else if !self.model.connected {
                    "正在连接 agent…"
                } else if self.model.is_viewing_history() {
                    "要求后续变更（将回到当前会话）…"
                } else {
                    "要求后续变更…  Enter 发送 · Shift+Enter 换行"
                };

                let edit = egui::TextEdit::multiline(&mut self.model.draft)
                    .desired_width(f32::INFINITY)
                    .desired_rows(2)
                    .frame(false)
                    .interactive(self.model.connected && !self.model.needs_login)
                    .hint_text(RichText::new(hint).color(MUTED));
                let response = ui.add(edit);

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
                ui.horizontal(|ui| {
                    ui.label(RichText::new("＋").size(16.0).color(MUTED));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_send = self.model.connected
                            && !self.model.busy
                            && !self.model.needs_login
                            && !self.model.draft.trim().is_empty();
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

                        if self.model.busy
                            && ui
                                .add(
                                    egui::Button::new(RichText::new("停止").size(12.0).color(TEXT))
                                        .fill(PANEL_2)
                                        .stroke(Stroke::new(1.0, BORDER))
                                        .corner_radius(CornerRadius::same(10))
                                        .min_size(Vec2::new(52.0, 28.0)),
                                )
                                .clicked()
                        {
                            self.send_cmd(UiCommand::Cancel);
                        }

                        let u = &self.model.usage.cumulative;
                        let usage_label = format!("Σ {}", format_tokens(u.total_tokens));
                        if soft_chip(ui, &usage_label, true) {
                            self.model.show_usage_detail = true;
                            self.model.show_user_menu = false;
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

                if menu_row(ui, "剩余用量", true) {
                    self.model.show_usage_detail = true;
                    self.model.show_user_menu = false;
                }
                if menu_row(ui, "设置 · 编辑 config.toml", false) {
                    self.model.show_user_menu = false;
                    if let Err(e) = crate::config_io::open_config_in_editor() {
                        self.model.apply(AgentEvent::Error(format!("无法打开配置: {e}")));
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

    /// Centered usage sheet: clean single card, tabs, no nested sidebar.
    fn usage_detail_window(&mut self, ctx: &egui::Context) {
        if !self.model.show_usage_detail {
            return;
        }

        let screen = ctx.screen_rect();
        let panel_w = (screen.width() * 0.52).clamp(520.0, 720.0);
        let panel_h = (screen.height() * 0.72).clamp(420.0, 640.0);

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
        let sess = self.model.usage.cumulative.clone();
        let sess_turns = self.model.usage.turns.len().max(turns.len());
        let mut open = true;
        let tab = self.model.usage_tab;

        egui::Window::new("剩余用量")
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
                        ui.label(
                            RichText::new("剩余用量").size(18.0).strong().color(TEXT),
                        );
                        ui.label(
                            RichText::new("按模型与对话轮次查看 token 消耗")
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
                    stat_chip(ui, "合计", &format_tokens(sess.total_tokens));
                    ui.add_space(8.0);
                    stat_chip(ui, "输入", &format_tokens(sess.input_tokens));
                    ui.add_space(8.0);
                    stat_chip(ui, "输出", &format_tokens(sess.output_tokens));
                    if let (Some(used), Some(size)) = (sess.context_used, sess.context_size) {
                        ui.add_space(8.0);
                        stat_chip(
                            ui,
                            "上下文",
                            &format!("{}/{}", format_tokens(used), format_tokens(size)),
                        );
                    }
                });

                ui.add_space(16.0);

                // Tabs
                ui.horizontal(|ui| {
                    if segment_tab(ui, "使用过的模型", tab == UsageTab::Models) {
                        self.model.usage_tab = UsageTab::Models;
                    }
                    ui.add_space(6.0);
                    if segment_tab(
                        ui,
                        &format!("对话轮次 ({})", turns.len()),
                        tab == UsageTab::Turns,
                    ) {
                        self.model.usage_tab = UsageTab::Turns;
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                let list_h = (panel_h - 200.0).max(180.0);
                egui::ScrollArea::vertical()
                    .id_salt("usage_sheet_scroll")
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.model.usage_tab {
                            UsageTab::Models => {
                                if model_stats.is_empty() {
                                    empty_hint(ui, "还没有模型用量。发送一条消息后会出现在这里。");
                                } else {
                                    for m in &model_stats {
                                        let pct = if sess.total_tokens > 0 {
                                            (m.total_tokens as f32 / sess.total_tokens as f32)
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
                                                    fill.set_width(
                                                        (rect.width() * pct).max(4.0),
                                                    );
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
                                        let expanded =
                                            self.model.is_history_expanded(&turn.id);
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
                                            ui.ctx().set_cursor_icon(
                                                egui::CursorIcon::PointingHand,
                                            );
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
                RichText::new("在当前工作区探索代码、改文件、跑工具。")
                    .size(14.0)
                    .color(MUTED),
            );
        });

        if self.model.needs_login {
            ui.add_space(20.0);
            Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(14))
                .stroke(Stroke::new(1.0, BORDER))
                .inner_margin(Margin::symmetric(18, 16))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(&self.model.login_message)
                            .size(13.5)
                            .color(MUTED),
                    );
                    ui.add_space(12.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("浏览器登录").color(BG).strong(),
                            )
                            .fill(ACCENT)
                            .corner_radius(CornerRadius::same(10))
                            .min_size(Vec2::new(160.0, 36.0)),
                        )
                        .clicked()
                    {
                        self.send_cmd(UiCommand::Login);
                    }
                });
            return;
        }

        ui.add_space(28.0);
        ui.label(RichText::new("快速开始").size(12.5).color(MUTED));
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
                            ui.label(RichText::new(&msg.text).size(14.5).color(TEXT));
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
                            let height = ui.available_height().max(40.0);
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(3.0, height.min(80.0)),
                                egui::Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(rect, CornerRadius::same(2), ACCENT_BAR);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                markdown::render(ui, &msg.text, TEXT);
                            });
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
                                            RichText::new(&m.id).size(11.5).monospace().color(MUTED),
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
                            self.model.apply(AgentEvent::Error(format!("无法打开配置: {e}")));
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
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("拒绝").color(TEXT))
                                .fill(PANEL_2)
                                .stroke(Stroke::new(1.0, BORDER))
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(90.0, 34.0)),
                        )
                        .clicked()
                    {
                        self.model.pending_permission = None;
                        self.send_cmd(UiCommand::PermissionResponse { allow: false });
                        self.model.status = "Ready".into();
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new(RichText::new("批准").color(BG).strong())
                                .fill(ACCENT)
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(100.0, 34.0)),
                        )
                        .clicked()
                    {
                        self.model.pending_permission = None;
                        self.send_cmd(UiCommand::PermissionResponse { allow: true });
                        self.model.status = "Working…".into();
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
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(11.0).color(MUTED));
                ui.label(RichText::new(value).size(14.0).strong().color(TEXT));
            });
        });
}

fn segment_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let fill = if selected { SELECTED } else { Color32::TRANSPARENT };
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

fn nav_item(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let fill = if selected { SELECTED } else { Color32::TRANSPARENT };
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
    ui.painter()
        .circle_filled(rect.center(), 14.0, AVATAR);
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
