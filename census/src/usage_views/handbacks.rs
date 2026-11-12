// SPDX-License-Identifier: MPL-2.0
//! The `census usage --handbacks` view: spawns and hand-backs per ISO week and per role.

use super::{hand_back_rate, rate, table};
use capsa_core::render::{Doc, Render};
use census::usage::{HandbackWeek, RoleHandbacks};
use serde::{Serialize, Serializer};

/// `census usage --handbacks`: spawns and hand-backs per ISO week and per role. Its JSON is the weeks
/// alone. The hand-back count is a heuristic, and the view says so.
pub(crate) struct HandbacksView<'a> {
    pub weeks: &'a [HandbackWeek],
    pub roles: &'a [RoleHandbacks],
}

impl Serialize for HandbacksView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.weeks.serialize(serializer)
    }
}

impl Render for HandbacksView<'_> {
    fn doc(&self, doc: &mut Doc) {
        let rows = self.weeks;
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
        doc.section("by role");
        doc.line(
            "a hand-back counts against the role of the worker spawn it followed; minutes run from that spawn's start to the senior-worker spawn's start",
        );
        let rows = self
            .roles
            .iter()
            .map(|r| {
                vec![
                    r.role.clone(),
                    r.spawns.to_string(),
                    r.handbacks.to_string(),
                    r.median_min
                        .map_or_else(|| "n/a".to_owned(), |m| format!("{m:.1}")),
                ]
            })
            .collect();
        doc.table(table(
            &["role", "spawns", "hand-backs", "median min"],
            "lrrr",
            rows,
        ));
    }
}
