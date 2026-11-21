// SPDX-License-Identifier: MPL-2.0
//! The JSON view of `acus diff`: the status header, then one object per changed file.

use super::report::Changes;
use super::{Args, status, total};
use serde_json::Value;

/// Prints the changed files as one JSON document with stable key order.
pub(super) fn print(a: &Args, c: &Changes) {
    let files: Vec<Value> = c
        .files
        .iter()
        .map(|f| {
            let symbols: Vec<Value> = f
                .symbols
                .iter()
                .map(|g| {
                    let mut s = serde_json::json!({
                        "symbol": g.label, "range": g.range, "added": g.add, "removed": g.del,
                    });
                    if a.lines {
                        s["lines"] = g.lines.iter().map(|(add, n, t)| serde_json::json!({
                            "op": if *add { "+" } else { "-" }, "line": n, "text": t,
                        })).collect();
                    }
                    s
                })
                .collect();
            serde_json::json!({
                "status": status(&f.diff), "path": f.diff.new.as_ref().or(f.diff.old.as_ref()), "old_path": f.diff.old,
                "binary": f.diff.binary, "added": total(&f.diff, true), "removed": total(&f.diff, false), "symbols": symbols,
            })
        })
        .collect();
    let mut root = serde_json::Map::new();
    if let Some(h) = &c.header {
        root.insert("header".into(), h.json());
    }
    root.insert("files".into(), Value::Array(files));
    outln!("{}", Value::Object(root));
}
