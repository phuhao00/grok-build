//! Load system CJK fonts so Chinese UI/chat text does not render as tofu/mojibake.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Prefer regular TTF/TTC faces known to work with egui/ab_glyph on Windows.
const CANDIDATES: &[(&str, &str, u32)] = &[
    // name, path, ttc index
    ("msyh", r"C:\Windows\Fonts\msyh.ttc", 0),
    ("msyhl", r"C:\Windows\Fonts\msyhl.ttc", 0),
    ("simhei", r"C:\Windows\Fonts\simhei.ttf", 0),
    ("simkai", r"C:\Windows\Fonts\simkai.ttf", 0),
    ("simsun", r"C:\Windows\Fonts\simsun.ttc", 0),
];

pub fn install(ctx: &egui::Context) {
    let Some((name, bytes, index)) = load_cjk_font() else {
        tracing::warn!("no CJK font found; Chinese text may render as tofu");
        return;
    };

    tracing::info!(font = %name, index, bytes = bytes.len(), "installed CJK UI font");

    let mut fonts = FontDefinitions::default();
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(data));

    // Highest priority for proportional so Han glyphs resolve (YaHei also covers Latin).
    if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
        fam.insert(0, name.to_owned());
    }
    // Fallback for monospace so code blocks can show Chinese comments/paths.
    if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
        fam.push(name.to_owned());
    }

    ctx.set_fonts(fonts);
}

fn load_cjk_font() -> Option<(&'static str, Vec<u8>, u32)> {
    for (name, path, index) in CANDIDATES {
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() > 4 && font_face_ok(&bytes) => {
                return Some((*name, bytes, *index));
            }
            Ok(_) => {
                tracing::warn!(path, "font file empty/invalid, trying next");
            }
            Err(e) => {
                tracing::debug!(path, error = %e, "font not readable");
            }
        }
    }
    None
}

fn font_face_ok(bytes: &[u8]) -> bool {
    let magic = &bytes[0..4];
    magic == b"ttcf"
        || magic == b"OTTO"
        || magic == b"true"
        || magic == [0, 1, 0, 0]
        || magic == b"typ1"
}
