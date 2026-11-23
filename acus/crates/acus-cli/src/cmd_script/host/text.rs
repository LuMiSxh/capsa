// SPDX-License-Identifier: MPL-2.0
//! Converts a script's TOML and YAML text to JSON, and diffs two texts line by line.

use acus_script::HostError;
use serde::Deserialize;
use serde_json::{Value, json};

/// Unchanged lines kept around each change when the caller gives no context.
const DIFF_CONTEXT: usize = 3;

/// Returns the JSON form of the TOML document `text`. A datetime becomes its RFC 3339 text.
pub(in super::super) fn parse_toml(text: &str) -> Result<Value, HostError> {
    let table: toml::Table =
        toml::from_str(text).map_err(|e| HostError::new(format!("cannot parse TOML: {e}")))?;
    Ok(from_toml(toml::Value::Table(table)))
}

/// Returns the JSON form of the first YAML document in `text`. Later documents are ignored, and
/// text with no document gives `null`.
pub(in super::super) fn parse_yaml(text: &str) -> Result<Value, HostError> {
    let Some(document) = serde_norway::Deserializer::from_str(text).next() else {
        return Ok(Value::Null);
    };
    Value::deserialize(document).map_err(|e| {
        HostError::new(format!("cannot parse YAML: {e}"))
            .with_hint("check the indentation, and that each key is followed by ': '")
    })
}

/// Returns the unified diff from `old` to `new`, with `context` unchanged lines around each change.
pub(in super::super) fn text_diff(old: &str, new: &str, context: Option<usize>) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(context.unwrap_or(DIFF_CONTEXT))
        .header("a", "b")
        .to_string()
}

/// Converts a TOML value to JSON. Non-finite floats become null, as `serde_json` does.
fn from_toml(value: toml::Value) -> Value {
    match value {
        toml::Value::String(text) => json!(text),
        toml::Value::Integer(number) => json!(number),
        toml::Value::Float(number) => json!(number),
        toml::Value::Boolean(flag) => json!(flag),
        toml::Value::Datetime(when) => json!(when.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(from_toml).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, item)| (key, from_toml(item)))
                .collect(),
        ),
    }
}
