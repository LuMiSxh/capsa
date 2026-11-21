// SPDX-License-Identifier: MPL-2.0
//! The `census usage --scripts` view: the runs of saved acus scripts per name and scope.

use super::table;
use capsa_core::render::{Doc, Render};
use census::usage::ScriptStat;
use serde::Serialize;

/// `census usage --scripts`: per script and scope its runs, failures, median duration, last use and
/// most common repository. Its JSON is the rows alone.
#[derive(Serialize)]
pub(crate) struct ScriptsView<'a> {
    #[serde(skip)]
    path: String,
    #[serde(skip)]
    now: u64,
    #[serde(skip)]
    runs: u64,
    #[serde(skip)]
    hidden: usize,
    scripts: Vec<&'a ScriptStat>,
}

impl<'a> ScriptsView<'a> {
    /// Builds the view of `rows`, keeping `top` of them.
    pub(crate) fn new(path: String, rows: &'a [ScriptStat], top: usize, now: u64) -> Self {
        Self {
            path,
            now,
            runs: rows.iter().map(|r| r.runs).sum(),
            hidden: rows.len().saturating_sub(top),
            scripts: rows.iter().take(top).collect(),
        }
    }
}

/// `35m`, `3h` or `2d` since `ts`.
fn age(now: u64, ts: u64) -> String {
    match now.saturating_sub(ts) {
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}

impl Render for ScriptsView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title(format!("scripts: {} runs ({})", self.runs, self.path));
        if self.runs == 0 {
            doc.hint("run a saved script first: acus script NAME");
            return;
        }
        let rows = self
            .scripts
            .iter()
            .map(|s| {
                vec![
                    s.name.clone(),
                    s.scope.clone(),
                    s.runs.to_string(),
                    s.failures.to_string(),
                    s.median_ms.to_string(),
                    age(self.now, s.last_ts),
                    s.repo.clone(),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "script",
                "scope",
                "runs",
                "fail",
                "median ms",
                "last",
                "repo",
            ],
            "llrrrrl",
            rows,
        ));
        if self.hidden > 0 {
            doc.more(self.hidden, "census usage --scripts --top N");
        }
    }
}
