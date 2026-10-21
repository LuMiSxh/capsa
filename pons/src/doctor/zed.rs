// SPDX-License-Identifier: MPL-2.0
//! Checks the Zed agent entry that runs pons.

use super::*;

/// Returns the path of Zed's `settings.json`.
pub fn settings_path(env: &Env) -> Option<PathBuf> {
    match env.platform {
        Platform::Windows => env
            .appdata
            .as_ref()
            .map(|a| a.join("Zed").join("settings.json")),
        _ => env
            .home
            .as_ref()
            .map(|h| h.join(".config").join("zed").join("settings.json")),
    }
}

/// Converts JSONC (comments, trailing commas) to plain JSON, leaving strings alone.
pub fn strip_jsonc(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    // Pass 1: comments.
    let mut plain = String::with_capacity(src.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            plain.push(c);
            if c == '\\' && i + 1 < chars.len() {
                plain.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            plain.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            plain.push(c);
        }
        i += 1;
    }
    // Pass 2: commas directly before a closing bracket.
    let chars: Vec<char> = plain.chars().collect();
    let mut out = String::with_capacity(plain.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// An `agent_servers` entry that runs pons.
#[derive(Debug, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// What the settings text says about pons.
#[derive(Debug, PartialEq, Eq)]
pub enum Settings {
    Servers(Vec<Server>),
    /// The text did not parse even leniently; `mentions` counts lines that
    /// contain the word `pons`.
    Unparsed {
        mentions: usize,
    },
}

pub(super) fn file_stem(command: &str) -> String {
    let base = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// Finds the `agent_servers` entries whose command (or an argument, for a
/// wrapper such as `sh -c`) is pons. Best effort, never an error.
pub fn find_pons_servers(text: &str) -> Settings {
    let Ok(json) = serde_json::from_str::<Value>(&strip_jsonc(text)) else {
        let mentions = text.lines().filter(|l| l.contains("pons")).count();
        return Settings::Unparsed { mentions };
    };
    let mut found = Vec::new();
    if let Some(servers) = json.get("agent_servers").and_then(Value::as_object) {
        for (name, entry) in servers {
            let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
            let args: Vec<String> = entry
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let runs_pons = file_stem(command) == "pons"
                || args
                    .iter()
                    .any(|a| a.split_whitespace().any(|w| file_stem(w) == "pons"));
            if runs_pons {
                found.push(Server {
                    name: name.clone(),
                    command: command.to_owned(),
                    args,
                });
            }
        }
    }
    // Keys come back in file order (serde_json preserve_order); keep them sorted by name.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Settings::Servers(found)
}

pub(super) fn zed_section(out: &mut String, r: &Report) {
    let _ = writeln!(out, "\nZed");
    match (&r.settings_path, &r.settings) {
        (None, _) => {
            let _ = writeln!(out, "  settings.json: location unknown (no home directory)");
        }
        (Some(path), None) => {
            let _ = writeln!(out, "  settings.json: {path} (not found or unreadable)");
        }
        (Some(path), Some(settings)) => {
            let _ = writeln!(out, "  settings.json: {path}");
            match settings {
                Settings::Servers(list) if list.is_empty() => {
                    let _ = writeln!(out, "  agent_servers running pons: none");
                }
                Settings::Servers(list) => {
                    for s in list {
                        let _ = writeln!(
                            out,
                            "  agent_servers running pons: {} (command {:?}, args {:?})",
                            s.name, s.command, s.args
                        );
                    }
                }
                Settings::Unparsed { mentions } => {
                    let _ = writeln!(
                        out,
                        "  could not parse settings.json; {mentions} line(s) mention pons"
                    );
                }
            }
        }
    }
    match &r.registry_adapter {
        Some((Some(v), dir)) => {
            let _ = writeln!(
                out,
                "  Zed registry adapter (a separate install, not what pons runs): claude-agent-acp {v} ({dir})"
            );
        }
        Some((None, dir)) => {
            let _ = writeln!(
                out,
                "  Zed registry adapter (a separate install, not what pons runs): not found under {dir}"
            );
        }
        None => {
            let _ = writeln!(out, "  Zed registry adapter: location unknown");
        }
    }
}
