//! Lightweight Markdown renderer for assistant messages (no extra deps).

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, RichText, Stroke, TextWrapMode, Vec2,
};

const CODE_BG: Color32 = Color32::from_rgb(30, 31, 36);
const CODE_FG: Color32 = Color32::from_rgb(210, 214, 220);
const INLINE_CODE_BG: Color32 = Color32::from_rgb(42, 44, 52);
const LINK_MUTED: Color32 = Color32::from_rgb(150, 150, 160);
const MARKER_W: f32 = 22.0;

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
            render_list_item(ui, indent_spaces(line), "•", rest, body);
            continue;
        }

        if let Some((num, rest)) = ordered_prefix(trimmed) {
            render_list_item(ui, indent_spaces(line), &format!("{num}."), rest, body);
            continue;
        }

        render_paragraph(ui, indent_spaces(line), trimmed, body);
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
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    Some((num, rest))
}

/// List row: marker on the left, content gets a full remaining-width wrap lane.
fn render_list_item(ui: &mut egui::Ui, indent: f32, marker: &str, rest: &str, body: Color32) {
    ui.horizontal_top(|ui| {
        ui.add_space(indent);
        ui.allocate_ui_with_layout(
            Vec2::new(MARKER_W, 18.0),
            egui::Layout::left_to_right(egui::Align::TOP),
            |ui| {
                ui.label(RichText::new(marker).size(14.5).color(LINK_MUTED));
            },
        );
        let width = ui.available_width().max(40.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 0.0),
            egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true),
            |ui| {
                ui.set_max_width(width);
                render_inline(ui, rest, body, 14.5, width);
            },
        );
    });
    ui.add_space(4.0);
}

fn render_paragraph(ui: &mut egui::Ui, indent: f32, text: &str, body: Color32) {
    let width = (ui.available_width() - indent).max(40.0);
    ui.horizontal_top(|ui| {
        ui.add_space(indent);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 0.0),
            egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true),
            |ui| {
                ui.set_max_width(width);
                render_inline(ui, text, body, 14.5, width);
            },
        );
    });
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

fn render_inline(ui: &mut egui::Ui, text: &str, body: Color32, size: f32, row_width: f32) {
    if let Some((prefix, paths)) = split_trailing_path_list(text) {
        let prefix = prefix
            .trim_end()
            .trim_end_matches("更改文件")
            .trim_end_matches(['：', ':', ' ', '\u{3000}']);
        if !prefix.is_empty() {
            render_inline_spans(ui, prefix, body, size, row_width);
        }
        ui.end_row();
        ui.add_space(4.0);
        ui.label(RichText::new("更改文件").size(12.0).color(LINK_MUTED));
        ui.end_row();
        ui.add_space(2.0);
        ui.allocate_ui_with_layout(
            Vec2::new(row_width, 0.0),
            egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true),
            |ui| {
                ui.set_max_width(row_width);
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                for path in paths {
                    render_code_pill(ui, path, size, row_width);
                }
            },
        );
        return;
    }

    render_inline_spans(ui, text, body, size, row_width);
}

struct CodeSpan<'a> {
    /// Byte index of opening backtick.
    start: usize,
    /// Byte index after closing backtick.
    end: usize,
    content: &'a str,
}

fn parse_code_spans(text: &str) -> Vec<CodeSpan<'_>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let content_start = i + 1;
        let Some(rel) = text[content_start..].find('`') else {
            break;
        };
        let content_end = content_start + rel;
        out.push(CodeSpan {
            start: i,
            end: content_end + 1,
            content: &text[content_start..content_end],
        });
        i = content_end + 1;
    }
    out
}

/// If the line ends with ≥2 path-like `` `...` `` chips separated only by commas,
/// return `(prefix_before_those_chips, paths)`.
fn split_trailing_path_list(text: &str) -> Option<(&str, Vec<&str>)> {
    let spans = parse_code_spans(text);
    if spans.len() < 2 {
        return None;
    }

    let mut run_start = spans.len();
    for idx in (0..spans.len()).rev() {
        if !looks_like_path(spans[idx].content) {
            break;
        }
        if idx + 1 < spans.len() {
            let between = text[spans[idx].end..spans[idx + 1].start].trim();
            if between != "," && between != "，" {
                break;
            }
        }
        // Trailing junk after the last chip must be empty / punctuation only.
        if idx + 1 == spans.len() {
            let after = text[spans[idx].end..].trim();
            if !after.is_empty() && after != "." && after != "。" {
                break;
            }
        }
        run_start = idx;
    }

    let path_count = spans.len() - run_start;
    if path_count < 2 {
        return None;
    }

    let prefix = text[..spans[run_start].start].trim_end();
    let paths = spans[run_start..]
        .iter()
        .map(|s| s.content)
        .collect::<Vec<_>>();
    Some((prefix, paths))
}

fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() > 260 || s.contains('\n') {
        return false;
    }
    // Prefer real paths; allow extension-only short names like `app.rs`.
    s.contains('/')
        || s.contains('\\')
        || (s.contains('.') && !s.chars().all(|c| c.is_ascii_hexdigit() || c == '.'))
}

fn render_inline_spans(ui: &mut egui::Ui, text: &str, body: Color32, size: f32, row_width: f32) {
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                let code = &after[..end];
                render_code_pill(ui, code, size, row_width);
                rest = &after[end + 1..];
                // Soft-skip a following comma — chips already separate visually.
                rest = rest
                    .strip_prefix(", ")
                    .or_else(|| rest.strip_prefix('，'))
                    .or_else(|| rest.strip_prefix(','))
                    .unwrap_or(rest);
                continue;
            }
        }

        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                let bold = &after[..end];
                ui.add(
                    egui::Label::new(RichText::new(bold).size(size).strong().color(body)).extend(),
                );
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
            ui.add(egui::Label::new(RichText::new(chunk).size(size).color(body)).wrap());
        }
        rest = &rest[next..];
        if next == 0 {
            let ch = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            ui.add(egui::Label::new(RichText::new(&rest[..ch]).size(size).color(body)).extend());
            rest = &rest[ch..];
        }
    }
}

/// Atomic inline-code chip: never wraps one glyph per line; truncates if wider than the row.
fn render_code_pill(ui: &mut egui::Ui, code: &str, size: f32, row_width: f32) {
    let pad_x = 10.0;
    let font_id = FontId::monospace((size - 1.0).max(11.0));
    let text_w = ui.fonts(|f| {
        f.layout_no_wrap(code.to_owned(), font_id.clone(), CODE_FG)
            .size()
            .x
    });
    let desired = text_w + pad_x;
    let max_pill = row_width.max(48.0);

    // If this chip won't fit the leftover of the current wrap line, start a new row first.
    let remaining = ui.available_width();
    if desired > remaining + 0.5 && remaining + 1.0 < max_pill {
        ui.end_row();
    }

    let avail = ui.available_width().min(max_pill).max(48.0);
    let truncating = desired > avail + 0.5;
    let pill_inner_w = if truncating {
        (avail - pad_x).max(24.0)
    } else {
        text_w
    };

    let response = egui::Frame::new()
        .fill(INLINE_CODE_BG)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.set_max_width(pill_inner_w);
            ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
            let label = RichText::new(code)
                .monospace()
                .size(size - 1.0)
                .color(CODE_FG);
            if truncating {
                ui.set_width(pill_inner_w);
                ui.add(egui::Label::new(label).truncate());
            } else {
                ui.add(egui::Label::new(label).extend());
            }
        })
        .response;

    if truncating {
        response.on_hover_text(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_path_list_keeps_commit_summary() {
        let text = "`e0a3c16` — 新增 Bony Monitor。更改文件：`crates/a/Cargo.toml`, `crates/a/src/lib.rs`, `README.md`";
        let (prefix, paths) = split_trailing_path_list(text).expect("path list");
        assert!(prefix.contains("e0a3c16"));
        assert!(prefix.contains("Bony Monitor"));
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], "crates/a/Cargo.toml");
    }

    #[test]
    fn short_hash_is_not_a_path() {
        assert!(!looks_like_path("e0a3c16"));
        assert!(looks_like_path("crates/foo.rs"));
        assert!(looks_like_path("README.md"));
    }

    #[test]
    fn two_inline_codes_without_paths_are_not_lists() {
        let text = "用 `foo` 和 `bar` 即可";
        assert!(split_trailing_path_list(text).is_none());
    }
}
