// SPDX-License-Identifier: MPL-2.0
//! Renders what a script printed and returned: strings as they are, a command's result as the
//! text its command prints, anything else as compact JSON, all within a cap.

use super::cap::{Cap, apply};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::render::{Doc, Render};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write;

/// Results whose text is the command's own printout, carried in a `text` field by the binding.
const TEXTED: &[&str] = &["outline", "follow", "diff", "log", "blame"];

/// The values a program printed, then the host calls that finished unawaited, then its return
/// value.
#[derive(Serialize)]
pub(super) struct Output {
    prints: Vec<Value>,
    /// Values of host calls that finished although the program failed before awaiting them.
    unawaited: Vec<Value>,
    result: Option<Value>,
    #[serde(skip)]
    cap: Option<Cap>,
}

impl Output {
    /// Returns the output of a run; `--json` (`format`) carries every value whole and is not capped.
    pub(super) fn new(
        prints: Vec<Value>,
        unawaited: Vec<Value>,
        result: Option<Value>,
        cap: Cap,
        format: Format,
    ) -> Self {
        let mut output = Output {
            prints,
            unawaited,
            result,
            cap: Some(cap),
        };
        if format == Format::Json {
            output.cap = None;
            let values = output
                .prints
                .iter_mut()
                .chain(&mut output.unawaited)
                .chain(&mut output.result);
            for value in values {
                if let Some(map) = value.as_object_mut().filter(|m| is_texted(m.get("kind"))) {
                    map.remove("text");
                }
            }
        }
        output
    }

    fn before(&self) -> impl Iterator<Item = &Value> {
        self.prints
            .iter()
            .chain(&self.unawaited)
            .filter(|v| !is_empty(v))
    }
}

fn is_texted(kind: Option<&Value>) -> bool {
    kind.and_then(Value::as_str)
        .is_some_and(|k| TEXTED.contains(&k))
}

fn lines<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<String> {
    values
        .flat_map(|v| text(v).lines().map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

impl Render for Output {
    fn doc(&self, doc: &mut Doc) {
        let before = lines(self.before());
        let last = lines(self.result.iter().filter(|v| !is_empty(v)));
        let all = match self.cap {
            Some(cap) => apply(&before, &last, cap),
            None => before.into_iter().chain(last).collect(),
        };
        for line in all {
            doc.line(line);
        }
    }

    fn outcome(&self) -> Outcome {
        let shown =
            self.before().next().is_some() || self.result.as_ref().is_some_and(|v| !is_empty(v));
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
    if is_texted(value.get("kind"))
        && let Some(shown) = value["text"].as_str()
    {
        return shown.to_owned();
    }
    let items = value["items"].as_array();
    let lines = match (value["kind"].as_str(), items) {
        (Some("hits"), Some(items)) => items.iter().map(hit).collect(),
        (Some("files"), Some(items)) => items.iter().map(file).collect(),
        (Some("views"), Some(items)) => items.iter().map(view).collect(),
        (Some("run"), _) => vec![run(value)],
        (Some("patch"), _) => patch(value),
        _ => vec![compact(value)],
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

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
