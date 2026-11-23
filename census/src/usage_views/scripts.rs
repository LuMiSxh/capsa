// SPDX-License-Identifier: MPL-2.0
//! The `census usage --scripts` view: the runs of saved acus scripts per name and scope, and the
//! inline runs apart from them.

use super::table;
use capsa_core::render::{Doc, Render};
use census::usage::{InlineStat, ScriptStat};
use serde::Serialize;

/// `census usage --scripts`: per script and scope its runs, failures, median duration, last use and
/// most common repository, then the inline runs. Its JSON is the rows and the inline counts.
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
    inline: InlineStat,
}

impl<'a> ScriptsView<'a> {
    /// Builds the view of `rows` and `inline`, keeping `top` of the rows.
    pub(crate) fn new(
        path: String,
        rows: &'a [ScriptStat],
        inline: InlineStat,
        top: usize,
        now: u64,
    ) -> Self {
        Self {
            path,
            now,
            runs: rows.iter().map(|r| r.runs).sum(),
            hidden: rows.len().saturating_sub(top),
            scripts: rows.iter().take(top).collect(),
            inline,
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
        doc.title(format!(
            "scripts: {} saved runs, {} inline runs ({})",
            self.runs, self.inline.runs, self.path
        ));
        if self.runs == 0 && self.inline.runs == 0 {
            doc.hint("run a saved script first: acus script NAME");
            return;
        }
        if self.runs > 0 {
            self.saved_table(doc);
        }
        if self.inline.runs > 0 {
            doc.line(format!(
                "inline: {} failed, median {} ms, {} host calls",
                self.inline.failures, self.inline.median_ms, self.inline.calls
            ));
        }
    }
}

impl ScriptsView<'_> {
    fn saved_table(&self, doc: &mut Doc) {
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
