// SPDX-License-Identifier: MPL-2.0
//! Describes one hook of a tool and finds, adds and replaces its entries in a settings document.

use serde_json::{Value, json};

use crate::claude_settings::{
    Step, event_entries, event_entries_mut, group, items, matcher_of, remove_hook,
};

/// One hook a tool registers: its event, matcher and command, and the claims that tell its entries
/// from other hooks.
///
/// `owns` claims every entry the tool has registered under the event, current and older.
/// `legacy` claims the older forms that `HookFile::add` replaces; it keeps the other entries.
#[derive(Debug, Clone, Copy)]
pub struct HookSpec {
    /// The agent event the hook runs on, such as `SessionStart`.
    pub event: &'static str,
    /// The matcher the tool registers; `None` runs the hook on every occurrence of the event.
    pub matcher: Option<&'static str>,
    /// The command the hook runs.
    pub command: &'static str,
    /// Claims the commands of the tool under this event, current and older.
    pub owns: fn(&str) -> bool,
    /// Claims the older commands that `HookFile::add` replaces.
    pub legacy: Option<fn(&str) -> bool>,
}

/// What a settings document holds for a `HookSpec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookState {
    /// The spec's command runs under the spec's matcher.
    Installed,
    /// The spec's command runs under a matcher the user chose; `None` is an entry with no matcher.
    CustomMatcher { matcher: Option<String> },
    /// An entry of the tool runs another command: an older form, or the same binary with other
    /// arguments.
    Outdated { command: String },
    /// The tool has no entry under the event.
    Missing,
}

/// Returns the state of the hook that `spec` describes in `v`.
///
/// An outdated entry is reported before a current one, so a document holding both names the entry
/// to replace.
pub(super) fn state(v: &Value, spec: &HookSpec) -> HookState {
    let entries = event_entries(v, spec.event);
    let outdated = entries
        .iter()
        .flat_map(items)
        .filter_map(command_of)
        .find(|c| *c != spec.command && (spec.owns)(c));
    if let Some(command) = outdated {
        return HookState::Outdated {
            command: command.to_owned(),
        };
    }
    let mut custom = None;
    for entry in entries {
        if !items(entry).any(|i| command_of(i) == Some(spec.command)) {
            continue;
        }
        if matcher_of(entry) == spec.matcher {
            return HookState::Installed;
        }
        custom.get_or_insert_with(|| HookState::CustomMatcher {
            matcher: matcher_of(entry).map(str::to_owned),
        });
    }
    custom.unwrap_or(HookState::Missing)
}

/// Adds the hook to a document that has no entry of the tool under its event.
///
/// Legacy entries are removed first and give `Step::Replace`. An entry the tool already has, under
/// any matcher and whatever its command, is kept and gives `Step::Set`. Foreign hooks stay in place.
pub(super) fn add(v: &mut Value, spec: &HookSpec) -> Step {
    let replaced = spec
        .legacy
        .is_some_and(|legacy| remove_hook(v, spec.event, legacy));
    let owned = event_entries(v, spec.event)
        .iter()
        .flat_map(items)
        .filter_map(command_of)
        .any(|c| (spec.owns)(c));
    if owned {
        return if replaced { Step::Replace } else { Step::Set };
    }
    let hook = json!({"type": "command", "command": spec.command});
    event_entries_mut(v, spec.event).push(group(spec.matcher, &hook));
    if replaced {
        Step::Replace
    } else {
        Step::Install
    }
}

fn command_of(item: &Value) -> Option<&str> {
    item.get("command").and_then(Value::as_str)
}
