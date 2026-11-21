// SPDX-License-Identifier: MPL-2.0
//! Renders what a script printed and returned: strings as they are, a command's result as the
//! lines its agent format uses, anything else as indented JSON.

use acus_script::Outcome as Finished;
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write;

/// The values a program printed, then its return value.
#[derive(Serialize)]
pub(super) struct Output {
    prints: Vec<Value>,
    result: Option<Value>,
}

impl Output {
    pub(super) fn new(finished: Finished) -> Self {
        Output {
            prints: finished.prints,
            result: finished.value,
        }
    }
}

impl Render for Output {
    fn doc(&self, doc: &mut Doc) {
        for value in self.prints.iter().chain(&self.result) {
            if !is_empty(value) {
                doc.line(text(value));
            }
        }
    }

    fn outcome(&self) -> Outcome {
        let shown = self.prints.iter().chain(&self.result).any(|v| !is_empty(v));
        if shown {
            Outcome::Found
        } else {
            Outcome::Empty
        }
    }
}

/// Reports whether a value has nothing to show: `null`, `""`, `[]` or a list result without items.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        _ => false,
    }
}

fn text(value: &Value) -> String {
    if let Value::String(s) = value {
        return s.clone();
    }
    let items = value["items"].as_array();
    let lines = match (value["kind"].as_str(), items) {
        (Some("hits"), Some(items)) => items.iter().map(hit).collect(),
        (Some("files"), Some(items)) => items.iter().map(file).collect(),
        (Some("views"), Some(items)) => items.iter().map(view).collect(),
        (Some("run"), _) => vec![run(value)],
        (Some("patch"), _) => patch(value),
        _ => vec![pretty(value)],
    };
    lines.join("\n")
}

fn hit(item: &Value) -> String {
    format!(
        "{}:{}: {}",
        item["path"].as_str().unwrap_or_default(),
        item["line"],
        item["text"].as_str().unwrap_or_default().trim_start()
    )
}

fn file(item: &Value) -> String {
    let path = item["path"].as_str().unwrap_or_default();
    match item.get("lines").or(item.get("count")) {
        Some(n) => format!("{path}  {n}"),
        None => path.to_owned(),
    }
}

fn view(item: &Value) -> String {
    let mut head = format!(
        "== {} {}-{}",
        item["path"].as_str().unwrap_or_default(),
        item["start"],
        item["end"]
    );
    if let Some(symbol) = item["symbol"].as_str() {
        let _ = write!(head, " {symbol}");
    }
    format!("{head}\n{}", item["text"].as_str().unwrap_or_default())
}

fn run(item: &Value) -> String {
    let mut shown = item["text"].as_str().unwrap_or_default().to_owned();
    if item["ok"] != true {
        let _ = write!(shown, "\nexit code {}", item["code"]);
    }
    shown
}

fn patch(item: &Value) -> Vec<String> {
    let strings = |key: &str| -> Vec<String> {
        item[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let mut lines = strings("changes");
    lines.extend(strings("notes"));
    if item.get("run").is_some() {
        lines.push(run(&item["run"]));
    }
    lines
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
