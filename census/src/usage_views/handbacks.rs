// SPDX-License-Identifier: MPL-2.0
//! The `census usage --handbacks` view: spawns and hand-backs per ISO week.

use super::{hand_back_rate, rate, table};
use capsa_core::render::{Doc, Render};
use census::usage::HandbackWeek;
use serde::Serialize;

/// `census usage --handbacks`: spawns and hand-backs per ISO week, as its JSON. The hand-back count
/// is a heuristic, and the view says so.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct HandbacksView<'a>(pub &'a [HandbackWeek]);

impl Render for HandbacksView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let rows = self.0;
        doc.title("census usage --handbacks");
        doc.line(
            "hand-back heuristic: a senior-worker spawn starting within 30 min after a worker spawn of the same session ended (or started) counts; rate = hand-backs / worker spawns",
        );
        let total = |f: fn(&HandbackWeek) -> u64| rows.iter().map(f).sum::<u64>();
        let mut body: Vec<Vec<String>> = rows
            .iter()
            .map(|w| {
                vec![
                    w.week.clone(),
                    w.workers.to_string(),
                    w.seniors.to_string(),
                    w.handbacks.to_string(),
                    rate(w.rate),
                ]
            })
            .collect();
        body.push(vec![
            "total".to_owned(),
            total(|w| w.workers).to_string(),
            total(|w| w.seniors).to_string(),
            total(|w| w.handbacks).to_string(),
            rate(hand_back_rate(rows)),
        ]);
        doc.table(table(
            &["week", "workers", "senior-workers", "hand-backs", "rate"],
            "lrrrr",
            body,
        ));
    }
}
