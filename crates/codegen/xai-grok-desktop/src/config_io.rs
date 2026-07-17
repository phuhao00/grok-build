//! Read/write `~/.grok/config.toml` helpers for the desktop model picker.

use std::path::PathBuf;

pub fn grok_config_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".grok").join("config.toml")
}

/// Persist `[models] default = "…"` (creates section if missing).
pub fn set_default_model(model_id: &str) -> Result<(), String> {
    let path = grok_config_path();
    let mut text = if path.is_file() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    let escaped = model_id.replace('\\', "\\\\").replace('"', "\\\"");
    let new_line = format!("default = \"{escaped}\"");

    if let Some(idx) = text.find("[models]") {
        let after = &text[idx + "[models]".len()..];
        let next_section = after.find("\n[").map(|i| idx + "[models]".len() + i);
        let section_end = next_section.unwrap_or(text.len());
        let section = &text[idx..section_end];
        let replaced = if let Some(cap) = find_default_line(section) {
            let abs_start = idx + cap.0;
            let abs_end = idx + cap.1;
            format!("{}{}{}", &text[..abs_start], new_line, &text[abs_end..])
        } else {
            // Insert right after [models]
            let insert_at = idx + "[models]".len();
            let mut out = String::new();
            out.push_str(&text[..insert_at]);
            out.push('\n');
            out.push_str(&new_line);
            if !text[insert_at..].starts_with('\n') {
                out.push('\n');
            }
            out.push_str(&text[insert_at..]);
            out
        };
        text = replaced;
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("\n[models]\n");
        text.push_str(&new_line);
        text.push('\n');
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

fn find_default_line(section: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    for line in section.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("default")
            && trimmed.contains('=')
            && !trimmed.starts_with('#')
        {
            let start = offset + (line.len() - line.trim_start().len());
            let end = offset + line.trim_end_matches(['\r', '\n']).len();
            return Some((start, end));
        }
        offset += line.len();
    }
    None
}

pub fn open_config_in_editor() -> Result<(), String> {
    let path = grok_config_path();
    if !path.is_file() {
        set_default_model("qwen-max")?;
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "xdg-open".into());
        std::process::Command::new(editor)
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
