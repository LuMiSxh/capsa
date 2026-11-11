// SPDX-License-Identifier: MPL-2.0
//! Classifies shell commands and built-in tool calls into adoption categories.

mod classify;
mod codex;
mod pipelines;
mod shape;
#[cfg(test)]
mod tests;

use serde_json::Value;

pub(crate) use classify::classify_shaped;
pub(crate) use codex::codex_commands;

/// The categories of one tool call, and the command that names each in `--misses`.
#[derive(Default)]
pub(crate) struct Hits {
    pub categories: Vec<&'static str>,
    pub commands: Vec<(&'static str, String)>,
}

impl Hits {
    fn named(category: &'static str, command: &str) -> Self {
        Hits {
            categories: vec![category],
            commands: vec![(category, command.to_owned())],
        }
    }

    fn shaped(pairs: Vec<(&'static str, Option<String>)>) -> Self {
        let mut hits = Hits::default();
        for (category, shape) in pairs {
            hits.categories.push(category);
            if let Some(shape) = shape {
                hits.commands.push((category, shape));
            }
        }
        hits
    }
}

/// Categories of one Claude tool call (`input` is the raw tool input JSON).
pub(crate) fn claude_tool(name: &str, input: &str) -> Hits {
    match name {
        "Read" => Hits::named("read", name),
        "Grep" | "Glob" => Hits::named("search", name),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Hits::named("edit-builtin", name),
        "Bash" => serde_json::from_str::<Value>(input)
            .ok()
            .and_then(|v| {
                let c = v.get("command")?.as_str()?;
                Some(Hits::shaped(classify_shaped(c)))
            })
            .unwrap_or_default(),
        _ => Hits::default(),
    }
}

/// Categories of one Codex tool call (`input` is its arguments or custom input).
pub(crate) fn codex_tool(name: &str, input: &str) -> Hits {
    if name == "apply_patch" {
        return Hits::named("edit-builtin", name);
    }
    let mut hits = Hits::default();
    for c in codex_commands(input) {
        for (category, shape) in classify_shaped(&c) {
            if hits.categories.contains(&category) {
                continue;
            }
            hits.categories.push(category);
            if let Some(shape) = shape {
                hits.commands.push((category, shape));
            }
        }
    }
    hits
}

/// The activity a category belongs to; shares are computed within a group.
pub fn group(category: &str) -> &'static str {
    match category {
        "acus:find" | "acus:outline" | "acus:show" | "search" | "read" => "explore",
        "acus:patch" | "edit-sed" | "edit-python" | "edit-builtin" | "edit-shell" => "edit",
        "acus:ctx" | "build-filtered" | "build-unfiltered" => "build",
        "acus:diff" | "diff" => "diff",
        _ => "other",
    }
}
