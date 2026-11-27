// SPDX-License-Identifier: MPL-2.0
//! The data functions a script can call besides the acus commands: files under the allowed roots,
//! environment variables, TOML, YAML, CSV and text diffs. Each returns a JSON value, or a
//! `HostError` with the fix in its hint.

mod fs;
#[cfg(test)]
mod tests;
mod text;

use serde_json::{Value, json};

pub(super) use fs::{exists, lexical, read, resolve, stat};
pub(super) use text::{parse_csv, parse_toml, parse_yaml, text_diff};

/// Words that make an environment variable value secret when they appear in its name.
const SECRET_WORDS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASS", "AUTH", "CREDENTIAL"];

/// The lines `read` returns. `offset` counts from 1 and defaults to the first line.
#[derive(Default)]
pub(super) struct ReadOptions {
    pub(super) offset: Option<usize>,
    pub(super) limit: Option<usize>,
    pub(super) numbered: bool,
}

/// Returns the value of the environment variable `name`, or null when it is unset. A name that
/// holds a secret word returns "<set>" in place of its value.
pub(super) fn env(name: &str) -> Value {
    if name.is_empty() || name.contains("=") || name.contains("\0") {
        return Value::Null;
    }
    masked(name, std::env::var(name).ok())
}

fn masked(name: &str, value: Option<String>) -> Value {
    match value {
        None => Value::Null,
        Some(_) if is_secret(name) => json!("<set>"),
        Some(value) => json!(value),
    }
}

fn is_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}
