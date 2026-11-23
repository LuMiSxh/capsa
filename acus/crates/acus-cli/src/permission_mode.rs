// SPDX-License-Identifier: MPL-2.0
//! Owns where acus keeps the Claude Code permission mode of each session, which the hooks record
//! and `script` and `batch` read to build the permissions they obey.

use capsa_core::claude_settings::permissions::{Mode, ModeStore, Permissions};
use serde_json::Value;
use std::path::PathBuf;

fn store() -> ModeStore {
    ModeStore::in_state_dir("acus")
}

/// Returns the permission rules for a run in `cwd`, following the mode recorded for the session
/// that `CLAUDE_CODE_SESSION_ID` names. Outside Claude Code, or before a hook has recorded a mode,
/// no mode is known and only deny and ask rules refuse.
pub fn permissions(cwd: PathBuf, strict: bool) -> Permissions {
    let mode = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .and_then(|session| store().read(&session));
    Permissions::new(cwd).with_mode(mode).with_strict(strict)
}

/// Records the `permission_mode` of a hook event under its `session_id`; an event without either,
/// or with a mode this version does not know, records nothing. A write only happens when the
/// mode changed, and a failure is ignored: a hook must never get in the way of the call.
pub fn record(event: &Value) {
    let (Some(session), Some(mode)) = (
        event["session_id"].as_str(),
        event["permission_mode"].as_str().and_then(Mode::parse),
    ) else {
        return;
    };
    let _ = store().record(session, mode);
}
