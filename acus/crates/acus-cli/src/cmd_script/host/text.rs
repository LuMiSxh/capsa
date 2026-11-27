// SPDX-License-Identifier: MPL-2.0
//! Converts a script's TOML, YAML and CSV text to JSON, and diffs two texts line by line.

use acus_script::HostError;
use serde::Deserialize;
use serde_json::{Map, Value, json};

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

/// Returns the CSV rows as objects keyed by the first row, or as arrays with `header: false`.
/// Quoted fields may hold the delimiter, a newline and doubled quotes. Blank lines are skipped.
pub(in super::super) fn parse_csv(
    text: &str,
    delimiter: &str,
    header: bool,
) -> Result<Value, HostError> {
    let mut chars = delimiter.chars();
    let (Some(delim), None) = (chars.next(), chars.next()) else {
        return Err(HostError::new("the delimiter must be one character")
            .with_hint("parse.csv(text, {delimiter: ';'})"));
    };
    let rows = csv_rows(text, delim)?;
    if !header {
        return Ok(Value::Array(
            rows.into_iter()
                .map(|row| Value::Array(row.into_iter().map(Value::String).collect()))
                .collect(),
        ));
    }
    let Some(head) = rows.first() else {
        return Ok(Value::Array(Vec::new()));
    };
    let mut keys: Vec<String> = Vec::with_capacity(head.len());
    for name in head {
        let mut key = name.clone();
        let mut n = 2;
        while keys.contains(&key) {
            key = format!("{name}_{n}");
            n += 1;
        }
        keys.push(key);
    }
    let out = rows
        .into_iter()
        .skip(1)
        .map(|row| {
            let mut object = Map::new();
            for (i, key) in keys.iter().enumerate() {
                object.insert(key.clone(), row.get(i).map_or(Value::Null, |v| json!(v)));
            }
            Value::Object(object)
        })
        .collect();
    Ok(Value::Array(out))
}

/// Splits CSV `text` into rows of fields, following RFC 4180 quoting. A quote that never closes
/// is an error, so a truncated file does not pass as data.
fn csv_rows(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, HostError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            c => field.push(c),
        }
    }
    if quoted {
        return Err(
            HostError::new("cannot parse CSV: a quoted field is not closed")
                .with_hint("close the field with a double quote, or double an inner quote"),
        );
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
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
