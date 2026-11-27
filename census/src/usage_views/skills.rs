// SPDX-License-Identifier: MPL-2.0
//! The `census usage --skills` view: skills loaded with the `Skill` tool or printed by a tool.

use super::{k, table};
use capsa_core::render::{Doc, Render};
use census::usage::SkillStat;
use serde::Serialize;

/// `census usage --skills`: per skill and source, its calls, sessions, roles and result size.
#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct SkillsView<'a>(pub &'a [SkillStat]);

impl Render for SkillsView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --skills");
        if self.0.is_empty() {
            doc.line("no Skill calls and no printed skill text");
            return;
        }
        let rows = self
            .0
            .iter()
            .map(|s| {
                vec![
                    s.skill.clone(),
                    s.source.to_owned(),
                    s.calls.to_string(),
                    s.sessions.to_string(),
                    s.roles.join(","),
                    k(s.result_chars as f64),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "skill",
                "source",
                "calls",
                "sessions",
                "roles",
                "result chars",
            ],
            "llrrrl",
            rows,
        ));
        doc.line(
            "source tool is a Skill call; print is `acus self skill` or `census self skill` through \
             Bash, named with its --topic",
        );
    }
}
