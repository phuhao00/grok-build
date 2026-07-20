//! Read/write `~/.grok/config.toml` helpers for the desktop model picker.

use std::path::PathBuf;

use crate::events::ModelChoice;

pub fn grok_config_path() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".grok").join("config.toml")
}

#[derive(Debug, Clone, Default)]
pub struct ConfigModels {
    pub default_id: Option<String>,
    pub models: Vec<ModelChoice>,
    /// Distinct `env_key` names referenced by `[model.*]` entries.
    pub env_keys: Vec<String>,
}

/// Parse model catalog + default from `~/.grok/config.toml` (no secrets returned).
pub fn load_models_catalog() -> ConfigModels {
    let path = grok_config_path();
    let Ok(text) = std::fs::read_to_string(path) else {
        return ConfigModels::default();
    };
    parse_models_catalog(&text)
}

fn parse_models_catalog(text: &str) -> ConfigModels {
    let mut out = ConfigModels::default();
    let mut section: Option<String> = None;
    let mut cur_id: Option<String> = None;
    let mut cur_name: Option<String> = None;
    let mut cur_env: Option<String> = None;
    let mut cur_base: Option<String> = None;

    let flush = |out: &mut ConfigModels,
                 cur_id: &mut Option<String>,
                 cur_name: &mut Option<String>,
                 cur_env: &mut Option<String>,
                 cur_base: &mut Option<String>| {
        if let Some(id) = cur_id.take() {
            let name = cur_name.take().unwrap_or_else(|| id.clone());
            let mut description = String::new();
            if let Some(base) = cur_base.take() {
                description = base;
            }
            if let Some(env) = cur_env.take() {
                if !out.env_keys.iter().any(|k| k == &env) {
                    out.env_keys.push(env.clone());
                }
                if !description.is_empty() {
                    description.push_str(" · ");
                }
                description.push_str(&format!("env:{env}"));
            }
            out.models.push(ModelChoice {
                id,
                name,
                description,
            });
        } else {
            cur_name.take();
            cur_env.take();
            cur_base.take();
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            flush(
                &mut out,
                &mut cur_id,
                &mut cur_name,
                &mut cur_env,
                &mut cur_base,
            );
            let name = &line[1..line.len() - 1];
            if let Some(rest) = name.strip_prefix("model.") {
                section = Some("model".into());
                cur_id = Some(rest.trim().to_string());
            } else if name == "models" {
                section = Some("models".into());
            } else {
                section = None;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = trim_toml_string(value.trim());
        match section.as_deref() {
            Some("models") if key == "default" => {
                out.default_id = Some(value);
            }
            Some("model") => match key {
                "name" => cur_name = Some(value),
                "env_key" => cur_env = Some(value),
                "base_url" => cur_base = Some(value),
                "api_key" => {
                    // Presence of inline key counts as usable; never store the value.
                    if !value.is_empty() {
                        cur_env = cur_env.or_else(|| Some("__inline_api_key__".into()));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    flush(
        &mut out,
        &mut cur_id,
        &mut cur_name,
        &mut cur_env,
        &mut cur_base,
    );

    if out.default_id.is_none() {
        out.default_id = out.models.first().map(|m| m.id.clone());
    }
    out
}

fn trim_toml_string(value: &str) -> String {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')) {
        return v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    // Strip inline comments for bare values.
    v.split('#').next().unwrap_or(v).trim().to_string()
}

/// Pull User/Machine env vars referenced by `[model.*] env_key` into the process
/// environment so the spawned `grok` agent can resolve BYOK credentials.
///
/// GUI / IDE launches on Windows often miss User-scoped variables.
pub fn hydrate_model_env_keys() -> usize {
    let catalog = load_models_catalog();
    let mut injected = 0usize;
    for key in &catalog.env_keys {
        if key == "__inline_api_key__" {
            continue;
        }
        if std::env::var_os(key).is_some_and(|v| !v.is_empty()) {
            continue;
        }
        if let Some(val) = lookup_os_env(key) {
            // SAFETY: single-threaded bridge startup; values come from the OS env store.
            unsafe { std::env::set_var(key, val) };
            injected += 1;
            tracing::info!(%key, "hydrated model env_key into process environment");
        }
    }
    injected
}

fn lookup_os_env(key: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::process::Command;
        // GUI launches often miss User-scoped vars; pull User then Machine.
        for scope in ["User", "Machine"] {
            let output = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!(
                        "[Environment]::GetEnvironmentVariable('{}','{}')",
                        key.replace('\'', "''"),
                        scope
                    ),
                ])
                .output()
                .ok()?;
            if !output.status.success() {
                continue;
            }
            let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = key;
        None
    }
}

/// True when config defines at least one model whose credentials resolve now.
pub fn has_usable_model_credentials() -> bool {
    let _ = hydrate_model_env_keys();
    let catalog = load_models_catalog();
    catalog.env_keys.iter().any(|key| {
        key == "__inline_api_key__" || std::env::var_os(key).is_some_and(|v| !v.is_empty())
    })
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
        if trimmed.starts_with("default") && trimmed.contains('=') && !trimmed.starts_with('#') {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_model_sections() {
        let text = r#"
[models]
default = "qwen-max"

[model.qwen-max]
model = "qwen-max"
name = "Qwen Max"
env_key = "DASHSCOPE_API_KEY"
base_url = "https://example.com/v1"

[model.kimi]
name = "Kimi"
env_key = "MOONSHOT_API_KEY"
"#;
        let cat = parse_models_catalog(text);
        assert_eq!(cat.default_id.as_deref(), Some("qwen-max"));
        assert_eq!(cat.models.len(), 2);
        assert_eq!(cat.models[0].name, "Qwen Max");
        assert!(cat.env_keys.contains(&"DASHSCOPE_API_KEY".into()));
        assert!(cat.env_keys.contains(&"MOONSHOT_API_KEY".into()));
    }
}
