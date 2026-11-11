// SPDX-License-Identifier: MPL-2.0
//! The `census usage --by` view: the expected cost of each bucket of the usage pattern.

use super::{table, usd};
use capsa_core::exit::Outcome;
use capsa_core::render::{Doc, Render};
use census::limits::{Kind, Pattern, Table as Buckets};
use serde::Serialize;

/// The row of `census usage --by`: the expected cost of each bucket of the usage pattern, as its JSON.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct ByView<'a>(pub &'a Buckets);

impl Render for ByView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let t = self.0;
        doc.title("census usage --by");
        if !t.has_data() {
            doc.line("no usage data yet: no transcript to read");
            return;
        }
        let kind = match t.kind {
            Kind::Weekday => "weekday",
            Kind::Hour => "hour of the day",
            Kind::Cycle => "hour since the weekly reset",
        };
        doc.row("basis").text(format!(
            "{:.1} weeks of local history; pattern {}",
            t.basis.weeks,
            t.basis.pattern.label()
        ));
        if t.kind == Kind::Cycle && t.basis.pattern != Pattern::SinceReset {
            doc.line("needs two weeks of weekly snapshots to map the reset; zero until then");
            return;
        }
        doc.section(format!("expected cost per {kind}"));
        let rows = t
            .rows
            .iter()
            .map(|b| {
                vec![
                    b.label.clone(),
                    usd(b.usd),
                    format!("{:.1} %", 100.0 * b.share),
                    b.hours.to_string(),
                ]
            })
            .collect();
        doc.table(table(&["bucket", "usd", "share", "hours"], "lrrr", rows));
    }

    fn outcome(&self) -> Outcome {
        if self.0.has_data() {
            Outcome::Found
        } else {
            Outcome::Empty
        }
    }
}
