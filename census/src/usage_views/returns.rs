// SPDX-License-Identifier: MPL-2.0
//! The `census usage --returns` view: what each subagent role returned to its parent.

use super::{k, table};
use capsa_core::render::{Doc, Render};
use census::usage::ReturnStat;
use serde::Serialize;

/// `census usage --returns`: per role, the launch result, the completion notification and the words
/// of the subagent's final message.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct ReturnsView<'a>(pub &'a [ReturnStat]);

impl Render for ReturnsView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --returns");
        if self.0.is_empty() {
            doc.line("no subagent spawns");
            return;
        }
        let rows = self
            .0
            .iter()
            .map(|r| {
                vec![
                    r.role.clone(),
                    r.calls.to_string(),
                    r.transcripts.to_string(),
                    k(r.launch_p50 as f64),
                    k(r.launch_p90 as f64),
                    r.notifications.to_string(),
                    k(r.notification_p50 as f64),
                    k(r.notification_p90 as f64),
                    k(r.final_words_p50 as f64),
                    k(r.final_words_p90 as f64),
                    k(r.final_words_total as f64),
                    k(r.output as f64),
                    format!("{:.1}%", 100.0 * r.thinking_share),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "role",
                "calls",
                "transcripts",
                "launch p50",
                "launch p90",
                "notifications",
                "notification p50",
                "notification p90",
                "words p50",
                "words p90",
                "words total",
                "output",
                "thinking%",
            ],
            "lrrrrrrrrrrrr",
            rows,
        ));
        doc.line(
            "launch is the Agent call's result; notification is the <result> of the completion \
             message that reached the main context; words are the subagent's last text",
        );
    }
}
