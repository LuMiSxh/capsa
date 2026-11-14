// SPDX-License-Identifier: MPL-2.0
//! Sets, removes and reads single keys in the `env` object of `settings.json`.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use super::{SettingsError, Step, put_key, take_object};

/// What `remove_env` did to an env key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Removal {
    /// The key held the expected value and was deleted.
    Removed,
    /// The key is not in `env`; nothing changes.
    Absent,
    /// The key holds another value, set by hand or by someone else, so it stays.
    Kept,
}

impl Removal {
    /// Whether applying this removal changes the settings.
    pub fn writes(self) -> bool {
        matches!(self, Self::Removed)
    }

    /// The report word for this removal; `dry_run` gives the wording for a dry run.
    pub fn text(self, dry_run: bool) -> &'static str {
        match (self, dry_run) {
            (Self::Removed, false) => "removed",
            (Self::Removed, true) => "would remove",
            (Self::Absent, _) => "absent",
            (Self::Kept, _) => "kept",
        }
    }
}

/// Returns the string value of `key` in `env`, or `None` when it is missing or not a string.
pub fn env_value<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get("env")?.get(key)?.as_str()
}

/// Makes `env` hold `value` under `key`, creating `env` when it is missing.
///
/// An equal value gives `Step::Set` and nothing changes. `v` should come from `read`, which
/// refuses an `env` that is not an object.
pub fn set_env(v: &mut Value, key: &str, value: &str) -> Step {
    if env_value(v, key) == Some(value) {
        return Step::Set;
    }
    let mut env = take_object(v, "env");
    let had = env.insert(key.to_owned(), json!(value)).is_some();
    put_key(v, "env", Value::Object(env));
    if had { Step::Replace } else { Step::Install }
}

/// Removes `key` from `env` when its value is `expected`, and removes `env` when it is left
/// empty. A different value is kept, so a value set by hand is never deleted.
pub fn remove_env(v: &mut Value, key: &str, expected: &str) -> Removal {
    let Some(env) = v.get_mut("env").and_then(Value::as_object_mut) else {
        return Removal::Absent;
    };
    let Some(current) = env.get(key) else {
        return Removal::Absent;
    };
    if current.as_str() != Some(expected) {
        return Removal::Kept;
    }
    env.remove(key);
    if env.is_empty()
        && let Some(root) = v.as_object_mut()
    {
        root.remove("env");
    }
    Removal::Removed
}

pub(super) fn check_env(value: &Value, path: &Path) -> Result<(), SettingsError> {
    match value.get("env") {
        Some(env) if !env.is_object() => Err(SettingsError::Shape {
            path: path.to_owned(),
            key: "env".to_owned(),
            expected: "an object",
        }),
        _ => Ok(()),
    }
}
