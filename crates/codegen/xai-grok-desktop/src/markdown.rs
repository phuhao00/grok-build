//! Lightweight Markdown renderer for assistant messages (no extra deps).

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

const CODE_BG: Color32 = Color32::from_rgb(30, 31, 36);
const CODE_FG: Color32 = Color32::from_rgb(210, 214, 220);
const INLINE_CODE_BG: Color32 = Color32::from_rgb(42, 44, 52);
const LINK_MUTED: Color32 = Color32::from_rgb(150, 150, 160);

pub fn render(ui: &mut egui::Ui, text: &str, body: Color32) {
    let mut in_fence = false;
    let mut fence_buf = String::new();

    for raw_line in text.split('\n') {
        let line = raw_line;
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            if in_fence {
                render_code_block(ui, &fence_buf);
                fence_buf.clear();
                in_fence = false;
            } else {
                in_fence = true;
            }
            continue;
        }

        if in_fence {
            if !fence_buf.is_empty() {
                fence_buf.push('\n');
            }
            fence_buf.push_str(line);
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(8.0);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("### ") {
            ui.add_space(6.0);
            ui.label(RichText::new(rest).size(15.0).strong().color(body));
            ui.add_space(2.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            ui.add_space(8.0);
            ui.label(RichText::new(rest).size(17.0).strong().color(body));
            ui.add_space(2.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            ui.add_space(10.0);
            ui.label(RichText::new(rest).size(19.0).strong().color(body));
            ui.add_space(4.0);
            continue;
        }

        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "));
        if let Some(rest) = bullet {
            ui.horizontal_top(|ui| {
                ui.add_space(indent_spaces(line));
                ui.label(RichText::new("•").color(LINK_MUTED));
                ui.horizontal_wrapped(|ui| {
                    render_inline(ui, rest, body, 14.5);
                });
            });
            continue;
        }

        if let Some((num, rest)) = ordered_prefix(trimmed) {
            ui.horizontal_top(|ui| {
                ui.add_space(indent_spaces(line));
                ui.label(RichText::new(format!("{num}.")).color(LINK_MUTED));
                ui.horizontal_wrapped(|ui| {
                    render_inline(ui, rest, body, 14.5);
                });
            });
            continue;
        }

        ui.horizontal_wrapped(|ui| {
            ui.add_space(indent_spaces(line));
            render_inline(ui, trimmed, body, 14.5);
        });
    }

    if in_fence && !fence_buf.is_empty() {
        render_code_block(ui, &fence_buf);
    }
}

fn indent_spaces(line: &str) -> f32 {
    let spaces = line.chars().take_while(|c| *c == ' ').count();
    (spaces as f32 / 2.0) * 10.0
}

fn ordered_prefix(line: &str) -> Option<(&str, &str)> {
    let digit_len = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }
    let (num, rest) = line.split_at(digit_len);
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some((num, rest))
}

fn render_code_block(ui: &mut egui::Ui, code: &str) {
    egui::Frame::new()
        .fill(CODE_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 10))
        .stroke(Stroke::new(1.0, Color32::from_rgb(50, 52, 60)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(RichText::new(code).monospace().size(12.5).color(CODE_FG)).wrap(),
            );
        });
    ui.add_space(6.0);
}

fn render_inline(ui: &mut egui::Ui, text: &str, body: Color32, size: f32) {
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                let code = &after[..end];
                egui::Frame::new()
                    .fill(INLINE_CODE_BG)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::symmetric(5, 1))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(code)
                                .monospace()
                                .size(size - 1.0)
                                .color(CODE_FG),
                        );
                    });
                rest = &after[end + 1..];
                continue;
            }
        }

        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                let bold = &after[..end];
                ui.label(RichText::new(bold).size(size).strong().color(body));
                rest = &after[end + 2..];
                continue;
            }
        }

        let next = rest
            .find('`')
            .into_iter()
            .chain(rest.find("**"))
            .min()
            .unwrap_or(rest.len());
        let chunk = &rest[..next];
        if !chunk.is_empty() {
            ui.label(RichText::new(chunk).size(size).color(body));
        }
        rest = &rest[next..];
        if next == 0 {
            let ch = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            ui.label(RichText::new(&rest[..ch]).size(size).color(body));
            rest = &rest[ch..];
        }
    }
}
