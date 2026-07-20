//! Refined usage charts for the usage-stats sheet.

use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};
use egui_plot::{
    Bar, BarChart, GridMark, Line, MarkerShape, Plot, PlotBounds, PlotPoints, Points,
    uniform_grid_spacer,
};

use crate::usage::{ModelUsageSummary, TurnRecord, format_tokens};

const TEXT: Color32 = Color32::from_rgb(236, 236, 240);
const MUTED: Color32 = Color32::from_rgb(148, 150, 160);
const CARD: Color32 = Color32::from_rgb(24, 24, 28);
const CARD_BORDER: Color32 = Color32::from_rgb(48, 50, 58);
const LEGEND_BG: Color32 = Color32::from_rgb(30, 31, 36);
const LINE_TOTAL: Color32 = Color32::from_rgb(130, 170, 255);
const LINE_IN: Color32 = Color32::from_rgb(120, 195, 145);
const LINE_OUT: Color32 = Color32::from_rgb(235, 170, 105);
const LINE_CUM: Color32 = Color32::from_rgb(190, 155, 255);
const BAR_COLORS: &[Color32] = &[
    Color32::from_rgb(120, 165, 255),
    Color32::from_rgb(120, 195, 145),
    Color32::from_rgb(235, 170, 105),
    Color32::from_rgb(190, 155, 255),
    Color32::from_rgb(235, 130, 150),
    Color32::from_rgb(110, 200, 200),
];

/// Draw usage analytics with roomy axes and legends outside the plot canvas.
pub fn draw_usage_charts(ui: &mut egui::Ui, turns: &[TurnRecord], models: &[ModelUsageSummary]) {
    let chrono: Vec<&TurnRecord> = turns.iter().collect();

    if chrono.is_empty() {
        chart_card(ui, "统计图", "暂无数据", |ui| {
            ui.label(
                RichText::new("发送几轮对话后，这里会显示折线与柱状趋势。")
                    .size(13.0)
                    .color(MUTED),
            );
        });
        return;
    }

    let n = chrono.len() as f64;
    let max_turn = chrono
        .iter()
        .map(|t| {
            t.usage_delta
                .total_tokens
                .max(t.usage_delta.input_tokens)
                .max(t.usage_delta.output_tokens)
        })
        .max()
        .unwrap_or(0) as f64;
    let mut cum_max = 0u64;
    for t in &chrono {
        cum_max = cum_max.saturating_add(t.usage_delta.total_tokens);
    }
    let y_turn = (max_turn * 1.18).max(10.0);
    let y_cum = (cum_max as f64 * 1.12).max(10.0);

    // —— Per-turn lines ——
    chart_card(
        ui,
        "每轮 Token",
        "横轴为轮次，纵轴为该轮消耗",
        |ui| {
            external_legend(
                ui,
                &[("合计", LINE_TOTAL), ("输入", LINE_IN), ("输出", LINE_OUT)],
            );
            ui.add_space(8.0);

            let totals: PlotPoints = chrono
                .iter()
                .enumerate()
                .map(|(i, t)| [i as f64 + 1.0, t.usage_delta.total_tokens as f64])
                .collect();
            let inputs: PlotPoints = chrono
                .iter()
                .enumerate()
                .map(|(i, t)| [i as f64 + 1.0, t.usage_delta.input_tokens as f64])
                .collect();
            let outputs: PlotPoints = chrono
                .iter()
                .enumerate()
                .map(|(i, t)| [i as f64 + 1.0, t.usage_delta.output_tokens as f64])
                .collect();
            let markers: PlotPoints = chrono
                .iter()
                .enumerate()
                .map(|(i, t)| [i as f64 + 1.0, t.usage_delta.total_tokens as f64])
                .collect();

            let x_min = 0.5_f64;
            let x_max = (n + 0.5).max(1.5);

            base_plot("usage_per_turn", 188.0, y_turn)
                .x_axis_formatter(turn_axis_fmt)
                .y_axis_formatter(token_axis_fmt)
                .label_formatter(|name, value| {
                    if name.is_empty() {
                        format!("第 {} 轮 · {}", value.x.round() as i64, fmt_token(value.y))
                    } else {
                        format!(
                            "{name} · 第 {} 轮 · {}",
                            value.x.round() as i64,
                            fmt_token(value.y)
                        )
                    }
                })
                .show(ui, |plot_ui| {
                    plot_ui
                        .set_plot_bounds(PlotBounds::from_min_max([x_min, 0.0], [x_max, y_turn]));
                    // Names kept for hover only; legend is drawn outside.
                    plot_ui.line(Line::new(totals).name("合计").color(LINE_TOTAL).width(2.0));
                    plot_ui.line(Line::new(inputs).name("输入").color(LINE_IN).width(1.5));
                    plot_ui.line(Line::new(outputs).name("输出").color(LINE_OUT).width(1.5));
                    plot_ui.points(
                        Points::new(markers)
                            .shape(MarkerShape::Circle)
                            .radius(3.2)
                            .filled(true)
                            .color(LINE_TOTAL),
                    );
                });
        },
    );

    ui.add_space(12.0);

    // —— Cumulative ——
    chart_card(ui, "累计消耗", "随轮次累加的 token 总量", |ui| {
        external_legend(ui, &[("累计 Σ", LINE_CUM)]);
        ui.add_space(8.0);

        let mut running = 0u64;
        let cumulative: PlotPoints = chrono
            .iter()
            .enumerate()
            .map(|(i, t)| {
                running = running.saturating_add(t.usage_delta.total_tokens);
                [i as f64 + 1.0, running as f64]
            })
            .collect();
        let x_min = 0.5_f64;
        let x_max = (n + 0.5).max(1.5);

        base_plot("usage_cumulative", 156.0, y_cum)
            .x_axis_formatter(turn_axis_fmt)
            .y_axis_formatter(token_axis_fmt)
            .label_formatter(|_name, value| {
                format!(
                    "第 {} 轮 · 累计 {}",
                    value.x.round() as i64,
                    fmt_token(value.y)
                )
            })
            .show(ui, |plot_ui| {
                plot_ui.set_plot_bounds(PlotBounds::from_min_max([x_min, 0.0], [x_max, y_cum]));
                plot_ui.line(
                    Line::new(cumulative)
                        .name("累计 Σ")
                        .color(LINE_CUM)
                        .width(2.2)
                        .fill(0.0)
                        .fill_alpha(0.14),
                );
            });
    });

    ui.add_space(12.0);

    // —— Model bars ——
    chart_card(ui, "模型用量对比", "各模型合计 token", |ui| {
        if models.is_empty() {
            ui.label(
                RichText::new("暂无按模型汇总的数据。")
                    .size(13.0)
                    .color(MUTED),
            );
            return;
        }

        let legend_items: Vec<(String, Color32)> = models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = if m.model_name.is_empty() {
                    m.model_id.clone()
                } else {
                    m.model_name.clone()
                };
                (
                    format!("{} · {}", name, format_tokens(m.total_tokens)),
                    BAR_COLORS[i % BAR_COLORS.len()],
                )
            })
            .collect();
        Frame::new()
            .fill(LEGEND_BG)
            .corner_radius(CornerRadius::same(8))
            .stroke(Stroke::new(1.0, CARD_BORDER))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, color) in &legend_items {
                        legend_dot(ui, color, label);
                        ui.add_space(12.0);
                    }
                });
            });
        ui.add_space(8.0);

        let max_bar = models.iter().map(|m| m.total_tokens).max().unwrap_or(0) as f64;
        let y_max = (max_bar * 1.2).max(10.0);
        let count = models.len() as f64;
        let bar_w = if count <= 1.0 { 0.45 } else { 0.62 };
        let x_pad = if count <= 1.0 { 0.9 } else { 0.7 };

        let bars: Vec<Bar> = models
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = if m.model_name.is_empty() {
                    m.model_id.as_str()
                } else {
                    m.model_name.as_str()
                };
                Bar::new(i as f64, m.total_tokens as f64)
                    .name(name.to_string())
                    .fill(BAR_COLORS[i % BAR_COLORS.len()])
                    .width(bar_w)
            })
            .collect();

        let model_names: Vec<String> = models
            .iter()
            .map(|m| {
                if m.model_name.is_empty() {
                    m.model_id.clone()
                } else {
                    m.model_name.clone()
                }
            })
            .collect();

        base_plot("usage_model_bars", 168.0, y_max)
            .x_axis_formatter(move |mark, _| {
                let i = mark.value.round() as isize;
                if i >= 0 && (i as usize) < model_names.len() {
                    truncate_label(&model_names[i as usize], 12)
                } else {
                    String::new()
                }
            })
            .y_axis_formatter(token_axis_fmt)
            .label_formatter(|name, value| {
                if name.is_empty() {
                    fmt_token(value.y)
                } else {
                    format!("{name} · {}", fmt_token(value.y))
                }
            })
            .show(ui, |plot_ui| {
                plot_ui.set_plot_bounds(PlotBounds::from_min_max(
                    [-x_pad, 0.0],
                    [(count - 1.0) + x_pad, y_max],
                ));
                plot_ui.bar_chart(BarChart::new(bars));
            });
    });
}

fn chart_card(ui: &mut egui::Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(1.0, CARD_BORDER))
        // Extra left padding so Y-axis labels never kiss the card edge.
        .inner_margin(Margin {
            left: 16,
            right: 14,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(13.5).strong().color(TEXT));
            ui.label(RichText::new(subtitle).size(11.5).color(MUTED));
            ui.add_space(8.0);
            add(ui);
        });
}

fn external_legend(ui: &mut egui::Ui, items: &[(&str, Color32)]) {
    Frame::new()
        .fill(LEGEND_BG)
        .corner_radius(CornerRadius::same(8))
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, (label, color)) in items.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(14.0);
                    }
                    legend_dot(ui, color, label);
                }
            });
        });
}

fn legend_dot(ui: &mut egui::Ui, color: &Color32, label: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, *color);
        ui.add_space(6.0);
        ui.label(RichText::new(label).size(12.5).color(TEXT));
    });
}

fn base_plot(id: impl std::hash::Hash, height: f32, y_max: f64) -> Plot<'static> {
    let major = nice_step(y_max / 4.0).max(1.0);
    Plot::new(id)
        .height(height)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_drag(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show_background(false)
        .show_axes([true, true])
        .show_grid([false, true])
        .clamp_grid(true)
        // Keep plot content away from axis labels.
        .set_margin_fraction(Vec2::new(0.06, 0.12))
        // Wide enough for labels like "25k" with CJK UI font metrics.
        .y_axis_min_width(56.0)
        .y_grid_spacer(uniform_grid_spacer(move |_input| {
            [major, major / 2.0, major / 5.0]
        }))
}

fn nice_step(raw: f64) -> f64 {
    if raw <= 0.0 {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let frac = raw / base;
    let nice = if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * base
}

fn turn_axis_fmt(mark: GridMark, _range: &std::ops::RangeInclusive<f64>) -> String {
    let v = mark.value;
    if (v - v.round()).abs() < 1e-6 && v >= 0.5 {
        format!("{}", v.round() as i64)
    } else {
        String::new()
    }
}

/// Compact axis ticks — avoid wide strings like "5.00k" that get clipped.
fn token_axis_fmt(mark: GridMark, _range: &std::ops::RangeInclusive<f64>) -> String {
    axis_token(mark.value)
}

fn axis_token(v: f64) -> String {
    let n = v.max(0.0);
    if n >= 1_000_000.0 {
        let m = n / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as u64)
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1000.0 {
        let k = n / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round() as u64)
        } else {
            format!("{k:.0}k")
        }
    } else {
        format!("{}", n.round() as u64)
    }
}

fn fmt_token(v: f64) -> String {
    let n = v.max(0.0) as u64;
    format_tokens(n)
}

fn truncate_label(s: &str, max: usize) -> String {
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
