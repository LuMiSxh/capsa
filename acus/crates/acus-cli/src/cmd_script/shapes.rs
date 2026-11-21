// SPDX-License-Identifier: MPL-2.0
//! Reshapes what the commands compute into the plain JavaScript values a script receives: an
//! object `{kind, items}` for a list, or an object with `kind` for a single result.

use crate::cmd_blame::BlameReport;
use crate::cmd_diff::DiffReport;
use crate::cmd_files::FilesReport;
use crate::cmd_find::FindReport;
use crate::cmd_follow::FollowReport;
use crate::cmd_log::LogReport;
use crate::cmd_outline::OutlineReport;
use crate::cmd_patch::PatchReport;
use crate::cmd_run::RunReport;
use crate::cmd_show::ShowReport;
use acus_script::HostError;
use acus_search::FileHits;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// A list result: the array a script sees carries `kind`.
pub(super) fn list(kind: &str, items: Vec<Value>) -> Value {
    json!({"kind": kind, "items": Value::Array(items)})
}

/// The serialized form of a report, whose private fields the shapes below read by name.
fn data<T: Serialize>(report: &T) -> Result<Value, HostError> {
    serde_json::to_value(report)
        .map_err(|e| HostError::new(format!("cannot read the command's result: {e}")))
}

/// Sets `key` to `value` on `item` unless it is `None`.
fn put(item: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        item.insert(key.to_owned(), value);
    }
}

fn symbol_at(file: &FileHits, line: usize) -> Option<&acus_syntax::Symbol> {
    file.outline.as_ref()?.enclosing(line)
}

/// `{path, line, symbol?, text}` for every hit.
pub(super) fn hits(report: &FindReport) -> Value {
    let mut items = Vec::new();
    for file in report.files() {
        for hit in &file.hits {
            let mut item = Map::new();
            item.insert("path".into(), json!(file.path));
            item.insert("line".into(), json!(hit.line));
            put(
                &mut item,
                "symbol",
                symbol_at(file, hit.line).map(|s| json!(s.qual)),
            );
            item.insert("text".into(), json!(hit.text));
            items.push(Value::Object(item));
        }
    }
    list("hits", items)
}

/// One view per enclosing symbol of the hits, or the hit with two lines around it where no symbol
/// encloses it, as `find --block` prints them.
pub(super) fn blocks(report: &FindReport) -> Value {
    let mut items = Vec::new();
    for file in report.files() {
        let lines: Vec<&str> = file.source.lines().collect();
        let mut done = BTreeSet::new();
        for hit in &file.hits {
            let (symbol, start, end) = match symbol_at(file, hit.line) {
                Some(s) => (Some(s.qual.as_str()), s.start_line, s.end_line),
                None => (
                    None,
                    hit.line.saturating_sub(2).max(1),
                    (hit.line + 2).min(lines.len()),
                ),
            };
            if !done.insert((start, end)) {
                continue;
            }
            let mut item = Map::new();
            item.insert("path".into(), json!(file.path));
            item.insert("start".into(), json!(start));
            item.insert("end".into(), json!(end));
            put(&mut item, "symbol", symbol.map(|s| json!(s)));
            let text = lines.get(start - 1..end.min(lines.len())).unwrap_or(&[]);
            item.insert("text".into(), json!(text.join("\n")));
            items.push(Value::Object(item));
        }
    }
    list("views", items)
}

/// `{path}` for every file with hits, or with `count` `{path, count}`.
pub(super) fn hit_files(report: &FindReport, count: bool) -> Value {
    let items = report
        .files()
        .iter()
        .zip(report.counts())
        .map(|(file, n)| {
            let mut item = json!({"path": file.path});
            if count {
                item["count"] = json!(n);
            }
            item
        })
        .collect();
    list("files", items)
}

/// `{path, start, end, symbol?, text}` for every address shown whole.
pub(super) fn views(report: &ShowReport) -> Result<Value, HostError> {
    let failures: Vec<String> = data(report)?["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["Error"].as_str().map(str::to_owned))
        .collect();
    if !failures.is_empty() {
        return Err(HostError::new(failures.join("\n")));
    }
    let items = report
        .entries
        .iter()
        .map(|entry| {
            let mut item = Map::new();
            item.insert("path".into(), json!(entry.path));
            item.insert("start".into(), json!(entry.start));
            item.insert("end".into(), json!(entry.end));
            put(&mut item, "symbol", entry.symbol.as_ref().map(|s| json!(s)));
            item.insert("text".into(), json!(entry.text));
            Value::Object(item)
        })
        .collect();
    Ok(list("views", items))
}

/// `{path, lang?, lines?, symbols: [{kind, name, qual, depth, start, end, detail?}]}` per file.
pub(super) fn outline(report: &OutlineReport) -> Result<Value, HostError> {
    let mut items = Vec::new();
    for file in data(report)?["files"].as_array().into_iter().flatten() {
        let mut item = Map::new();
        item.insert("path".into(), file["path"].clone());
        let outline = &file["outline"];
        put(&mut item, "lang", outline.get("lang").cloned());
        put(&mut item, "lines", outline.get("lines").cloned());
        let symbols = outline["symbols"].as_array().into_iter().flatten();
        let symbols = symbols.map(|s| {
            let mut symbol = Map::new();
            for key in ["kind", "name", "qual", "depth"] {
                symbol.insert(key.into(), s[key].clone());
            }
            symbol.insert("start".into(), s["start_line"].clone());
            symbol.insert("end".into(), s["end_line"].clone());
            put(&mut symbol, "detail", s.get("detail").cloned());
            Value::Object(symbol)
        });
        item.insert("symbols".into(), Value::Array(symbols.collect()));
        items.push(Value::Object(item));
    }
    Ok(list("outline", items))
}

/// `{path, lines?}` per file; `lines` is the line count when `lines: true` and the file is text.
pub(super) fn files(report: &FilesReport) -> Result<Value, HostError> {
    let doc = data(report)?;
    let counts = doc["counts"].as_array();
    let items = doc["files"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, path)| {
            let mut item = json!({"path": path});
            if let Some(n) = counts.and_then(|c| c.get(i)).filter(|n| !n.is_null()) {
                item["lines"] = n.clone();
            }
            item
        })
        .collect();
    Ok(list("files", items))
}

/// `{from, to, lines, text, sure, depth}` per edge of the call tree.
pub(super) fn follow(report: &FollowReport) -> Result<Value, HostError> {
    let edges = data(report)?["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(list("follow", edges))
}

/// Per changed file `{path, status, add, del, symbols: [{symbol, start?, end?, add, del, lines}]}`
/// with `lines` as `{n, added, text}`; with `conflicts` per unmerged path `{path, ranges}`.
pub(super) fn diff(report: &DiffReport) -> Result<Value, HostError> {
    let doc = data(report)?;
    if let Some(unmerged) = doc["Conflicts"]["unmerged"].as_array() {
        return Ok(list("conflicts", unmerged.clone()));
    }
    let items = doc["Changes"]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(changed_file)
        .collect();
    Ok(list("diff", items))
}

fn changed_file(file: &Value) -> Value {
    let diff = &file["diff"];
    let path = diff["new"]
        .as_str()
        .or(diff["old"].as_str())
        .unwrap_or_default();
    let status = if diff["unmerged"] == true {
        "U"
    } else if diff["untracked"] == true {
        "?"
    } else if diff["old"].is_null() {
        "A"
    } else if diff["new"].is_null() {
        "D"
    } else if diff["old"] != diff["new"] {
        "R"
    } else {
        "M"
    };
    let symbols: Vec<Value> = file["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|group| {
            let mut symbol = Map::new();
            symbol.insert("symbol".into(), group["label"].clone());
            put(&mut symbol, "start", group["range"].get(0).cloned());
            put(&mut symbol, "end", group["range"].get(1).cloned());
            symbol.insert("add".into(), group["add"].clone());
            symbol.insert("del".into(), group["del"].clone());
            let lines = group["lines"].as_array().into_iter().flatten();
            let lines = lines.map(|l| json!({"n": l[1], "added": l[0], "text": l[2]}));
            symbol.insert("lines".into(), Value::Array(lines.collect()));
            Value::Object(symbol)
        })
        .collect();
    let total = |key: &str| -> u64 { symbols.iter().filter_map(|s| s[key].as_u64()).sum() };
    json!({
        "path": path, "status": status, "add": total("add"), "del": total("del"), "symbols": symbols
    })
}

/// `{hash, date, author, subject, files, add, del}` per commit.
pub(super) fn log(report: &LogReport) -> Result<Value, HostError> {
    let commits = data(report)?["commits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(list("log", commits))
}

/// `{start, end, commit, author, date, summary}` per run of lines from one commit.
pub(super) fn blame(report: &BlameReport) -> Result<Value, HostError> {
    let lines = data(report)?["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(list("blame", lines))
}

/// `{kind: "patch", check, written, changes, notes, regions: [{path, start, lines}], diffs}`.
pub(super) fn patch(report: &PatchReport) -> Result<Value, HostError> {
    let doc = data(report)?;
    let regions: Vec<Value> = doc["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| json!({"path": r["path"], "start": r["start"], "lines": r["lines"]}))
        .collect();
    Ok(json!({
        "kind": "patch", "check": doc["check"], "written": doc["written"],
        "changes": doc["changes"], "notes": doc["notes"], "regions": regions,
        "diffs": doc["diffs"],
    }))
}

/// `{kind: "run", ok, code, text, failures?}`; `failures` holds the code at each reference as
/// `{address, start, text}`.
pub(super) fn run(report: &RunReport) -> Result<Value, HostError> {
    let doc = data(report)?;
    let code = doc["code"].as_i64().unwrap_or(1);
    let mut result = json!({
        "kind": "run", "ok": code == 0, "code": code,
        "text": doc["json"]["shown"].as_str().unwrap_or_default(),
    });
    if let Some(locations) = doc["json"]["locations"]
        .as_array()
        .filter(|l| !l.is_empty())
    {
        result["failures"] = Value::Array(locations.clone());
    }
    Ok(result)
}
