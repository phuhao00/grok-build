//! Chat-first egui shell: left-aligned timeline, markdown, bottom composer.

use std::sync::mpsc;

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Vec2};
use tokio::sync::mpsc as tokio_mpsc;

use crate::agent_bridge::{self, BridgeConfig};
use crate::events::{AgentEvent, UiCommand};
use crate::markdown;
use crate::model::{AppModel, Role, TimelineItem};

const BG: Color32 = Color32::from_rgb(16, 17, 20);
const PANEL: Color32 = Color32::from_rgb(26, 27, 32);
const PANEL_2: Color32 = Color32::from_rgb(34, 36, 42);
const BORDER: Color32 = Color32::from_rgb(52, 54, 62);
const TEXT: Color32 = Color32::from_rgb(236, 236, 240);
const MUTED: Color32 = Color32::from_rgb(148, 150, 160);
const ACCENT: Color32 = Color32::from_rgb(245, 245, 247);
const USER_BG: Color32 = Color32::from_rgb(48, 52, 64);
const ASSIST_BG: Color32 = Color32::from_rgb(24, 25, 30);
const TOOL_BG: Color32 = Color32::from_rgb(28, 30, 36);
const DANGER: Color32 = Color32::from_rgb(220, 90, 90);
const OK: Color32 = Color32::from_rgb(110, 190, 130);
const ACCENT_BAR: Color32 = Color32::from_rgb(120, 160, 255);
const MAX_CHAT_W: f32 = 820.0;

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

        egui::TopBottomPanel::top("top_bar")
            .frame(
                Frame::NONE
                    .fill(BG)
                    .inner_margin(Margin::symmetric(20, 12))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ctx, |ui| {
                self.top_bar(ui);
            });

        egui::TopBottomPanel::bottom("composer")
            .frame(
                Frame::NONE
                    .fill(BG)
                    .inner_margin(Margin::symmetric(20, 14)),
            )
            .exact_height(148.0)
            .show(ctx, |ui| {
                chat_column(ui, |ui| {
                    self.composer(ui);
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG).inner_margin(Margin::symmetric(20, 8)))
            .show(ctx, |ui| {
                // ScrollArea must own the full panel width; shrink-to-content
                // was leaving a thin strip + scrollbar that looked like a blocker.
                egui::ScrollArea::vertical()
                    .id_salt("chat_scroll")
                    .stick_to_bottom(self.model.auto_scroll)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        chat_column(ui, |ui| {
                            if self.model.is_empty_chat() {
                                self.empty_state(ui);
                            } else {
                                self.timeline(ui);
                            }
                            ui.add_space(28.0);
                        });
                    });
            });

        self.permission_modal(ctx);
        self.model_picker_modal(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.send_cmd(UiCommand::Shutdown);
    }
}

impl BonyBuildApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        chat_column(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Bony Build").size(17.0).strong().color(TEXT));
                ui.add_space(10.0);

                let folder = self
                    .model
                    .cwd
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "workspace".into());

                chip(ui, &folder, MUTED);
                ui.add_space(6.0);
                if model_chip(
                    ui,
                    &self.model.current_model_name,
                    self.model.connected && !self.model.needs_login,
                ) {
                    self.model.show_model_picker = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.model.needs_login
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new("登录").size(12.0).color(BG).strong(),
                                )
                                .fill(ACCENT)
                                .corner_radius(CornerRadius::same(8))
                                .min_size(Vec2::new(64.0, 26.0)),
                            )
                            .clicked()
                    {
                        self.send_cmd(UiCommand::Login);
                    }

                    if self.model.busy {
                        ui.spinner();
                        ui.add_space(6.0);
                    }

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
                });
            });
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.add_space(56.0);
        ui.label(
            RichText::new("今天想做什么？")
                .size(28.0)
                .strong()
                .color(TEXT),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new("在当前工作区探索代码、改文件、跑工具。Enter 发送，Shift+Enter 换行。")
                .size(14.0)
                .color(MUTED),
        );

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
                ui.label(RichText::new("助手").size(12.0).strong().color(MUTED));
                ui.add_space(6.0);
                Frame::new()
                    .fill(ASSIST_BG)
                    .corner_radius(CornerRadius::same(12))
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let height = ui.available_height().max(40.0);
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::new(3.0, height.min(80.0)), egui::Sense::hover());
                            ui.painter()
                                .rect_filled(rect, CornerRadius::same(2), ACCENT_BAR);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                markdown::render(ui, &msg.text, TEXT);
                            });
                        });
                    });
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

    fn composer(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(1.0, BORDER))
            .shadow(Shadow {
                offset: [0, 4],
                blur: 20,
                spread: 0,
                color: Color32::from_black_alpha(90),
            })
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let hint = if self.model.needs_login {
                    "请先登录或配置 API Key…"
                } else if !self.model.connected {
                    "正在连接 agent…"
                } else {
                    "描述任务…  Enter 发送 · Shift+Enter 换行"
                };

                let edit = egui::TextEdit::multiline(&mut self.model.draft)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3)
                    .frame(false)
                    .interactive(self.model.connected && !self.model.needs_login)
                    .hint_text(RichText::new(hint).color(MUTED));
                let response = ui.add(edit);

                // Enter sends; Shift+Enter inserts newline (default multiline behavior).
                let enter_send = response.has_focus()
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Enter)
                            && !i.modifiers.shift
                            && !i.modifiers.ctrl
                            && !i.modifiers.command
                    });
                if enter_send {
                    // Consume the newline egui would otherwise insert.
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
                    ui.label(RichText::new("模型").size(11.5).color(MUTED));
                    ui.add_space(6.0);
                    if model_chip(
                        ui,
                        if self.model.current_model_name.is_empty() {
                            "选择模型"
                        } else {
                            &self.model.current_model_name
                        },
                        self.model.connected && !self.model.needs_login,
                    ) {
                        self.model.show_model_picker = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_send = self.model.connected
                            && !self.model.busy
                            && !self.model.needs_login
                            && !self.model.draft.trim().is_empty();
                        let send_btn = ui.add_enabled(
                            can_send,
                            egui::Button::new(RichText::new("发送").color(BG).strong())
                                .fill(ACCENT)
                                .corner_radius(CornerRadius::same(10))
                                .min_size(Vec2::new(72.0, 30.0)),
                        );
                        if send_btn.clicked() {
                            self.send_prompt();
                        }

                        if self.model.busy
                            && ui
                                .add(
                                    egui::Button::new(RichText::new("停止").color(TEXT))
                                        .fill(PANEL_2)
                                        .stroke(Stroke::new(1.0, BORDER))
                                        .corner_radius(CornerRadius::same(10))
                                        .min_size(Vec2::new(64.0, 30.0)),
                                )
                                .clicked()
                        {
                            self.send_cmd(UiCommand::Cancel);
                        }
                    });
                });
            });
    }

    fn model_picker_modal(&mut self, ctx: &egui::Context) {
        if !self.model.show_model_picker {
            return;
        }

        egui::Area::new(egui::Id::new("model_dim"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(160));
                if resp.clicked() {
                    self.model.show_model_picker = false;
                }
            });

        let mut open = true;
        egui::Window::new("选择模型")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
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
                ui.label(
                    RichText::new("当前会话模型（同时写入 ~/.grok/config.toml 默认值）")
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);

                if self.model.available_models.is_empty() {
                    ui.label(
                        RichText::new("暂无可用模型列表。可编辑配置文件添加 [model.*]。")
                            .size(13.0)
                            .color(MUTED),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            let models = self.model.available_models.clone();
                            let current = self.model.current_model_id.clone();
                            let busy = self.model.busy;
                            for m in models {
                                let selected = m.id == current;
                                let fill = if selected { PANEL_2 } else { TOOL_BG };
                                let stroke = if selected {
                                    Stroke::new(1.5, ACCENT_BAR)
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
                                                    RichText::new("当前")
                                                        .size(11.5)
                                                        .color(OK),
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
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
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

/// Center a fixed-width column, but keep content left-aligned and force the
/// full column width (never shrink to the text width).
fn chat_column(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width().min(MAX_CHAT_W);
    ui.vertical_centered(|ui| {
        ui.set_width(width);
        ui.with_layout(
            egui::Layout::top_down(egui::Align::LEFT).with_cross_justify(true),
            |ui| {
                ui.set_min_width(width);
                ui.set_max_width(width);
                add(ui);
            },
        );
    });
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(PANEL)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 4))
        .stroke(Stroke::new(1.0, BORDER))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(color));
        });
}

/// Clickable model chip. Returns true when clicked.
fn model_chip(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let color = if enabled { TEXT } else { MUTED };
    let resp = Frame::new()
        .fill(PANEL_2)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 4))
        .stroke(Stroke::new(1.0, ACCENT_BAR))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("▾ {text}")).size(12.0).color(color));
        })
        .response
        .interact(egui::Sense::click());
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
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
    // Thicker, more visible scrollbar.
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
