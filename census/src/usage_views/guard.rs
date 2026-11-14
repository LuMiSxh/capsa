// SPDX-License-Identifier: MPL-2.0
//! The `census usage --guard` view: the guard's decisions per rule or per project, with the follow
//! rate of each rule, then the latest escaped calls.

use super::{rate, table};
use capsa_core::render::{Doc, Render};
use census::usage::{
    GuardEntry, MIN_SAMPLE, Tally, WINDOW_SECS, by_project, by_rule, follow_rates,
};
use serde::Serialize;

/// One rule of `census usage --guard`: its decisions, and how often the agent followed them.
#[derive(Serialize)]
struct RuleCount {
    rule: String,
    #[serde(flatten)]
    tally: Tally,
    /// The decisions with a full follow window, see `follow_rates`.
    observed: u64,
    /// The share of the observed decisions that were not repeated; `None` below `MIN_SAMPLE`.
    follow: Option<f32>,
}

/// One rule of one project, as `--by-project` lists it.
#[derive(Serialize)]
struct ProjectCount {
    project: String,
    rule: String,
    #[serde(flatten)]
    tally: Tally,
}

/// `census usage --guard`: per rule how often it refused, hinted, was escaped or rewrote, and how
/// often the agent followed it. With `--by-project`, the same per project and rule. Then the latest
/// escaped calls, since they point at refusals acus could not replace.
#[derive(Serialize)]
pub(crate) struct GuardView<'a> {
    #[serde(skip)]
    path: String,
    #[serde(skip)]
    decisions: usize,
    rules: Vec<RuleCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projects: Option<Vec<ProjectCount>>,
    escaped: Vec<&'a GuardEntry>,
    #[serde(skip)]
    escaped_hidden: usize,
    #[serde(skip)]
    projects_hidden: usize,
}

impl<'a> GuardView<'a> {
    /// Builds the view of `entries`, keeping `top` rows of each table and the latest escaped calls.
    pub(crate) fn new(
        path: String,
        entries: &'a [GuardEntry],
        top: usize,
        per_project: bool,
    ) -> Self {
        let follow = follow_rates(entries);
        let rules = by_rule(entries)
            .into_iter()
            .map(|(rule, tally)| {
                let f = follow.get(rule).copied().unwrap_or_default();
                RuleCount {
                    rule: rule.to_owned(),
                    tally,
                    observed: f.observed,
                    follow: f.rate(),
                }
            })
            .collect();
        let escapes = entries.iter().filter(|e| e.kind == "escape").count();
        let escaped: Vec<&GuardEntry> = entries
            .iter()
            .filter(|e| e.kind == "escape")
            .rev()
            .take(top)
            .collect();
        let (projects, projects_hidden) = if per_project {
            let rows = by_project(entries);
            let hidden = rows.len().saturating_sub(top);
            let rows = rows
                .into_iter()
                .take(top)
                .map(|r| ProjectCount {
                    project: r.project,
                    rule: r.rule,
                    tally: r.tally,
                })
                .collect();
            (Some(rows), hidden)
        } else {
            (None, 0)
        };
        Self {
            path,
            decisions: entries.len(),
            rules,
            projects,
            escaped_hidden: escapes - escaped.len(),
            escaped,
            projects_hidden,
        }
    }

    fn rules_table(&self, doc: &mut Doc) {
        let rows = self
            .rules
            .iter()
            .map(|r| {
                vec![
                    r.rule.clone(),
                    r.tally.deny.to_string(),
                    r.tally.hint.to_string(),
                    r.tally.escape.to_string(),
                    r.tally.rewrite.to_string(),
                    r.observed.to_string(),
                    rate(r.follow.map(f64::from)),
                ]
            })
            .collect();
        doc.table(table(
            &[
                "rule", "deny", "hint", "escape", "rewrite", "observed", "follow",
            ],
            "lrrrrrr",
            rows,
        ));
        doc.line(format!(
            "follow: the share of observed refusals, hints and rewrites that the rule did not repeat in the same session within {} min; n/a below {MIN_SAMPLE} observed",
            WINDOW_SECS / 60
        ));
    }

    fn projects_table(&self, doc: &mut Doc, rows: &[ProjectCount]) {
        let rows = rows
            .iter()
            .map(|p| {
                vec![
                    p.project.clone(),
                    p.rule.clone(),
                    p.tally.deny.to_string(),
                    p.tally.hint.to_string(),
                    p.tally.escape.to_string(),
                    p.tally.rewrite.to_string(),
                ]
            })
            .collect();
        doc.table(table(
            &["project", "rule", "deny", "hint", "escape", "rewrite"],
            "llrrrr",
            rows,
        ));
        if self.projects_hidden > 0 {
            doc.more(
                self.projects_hidden,
                "census usage --guard --by-project --top N",
            );
        }
    }
}

impl Render for GuardView<'_> {
    fn doc(&self, doc: &mut Doc) {
        doc.title(format!(
            "guard: {} decisions ({})",
            self.decisions, self.path
        ));
        if self.decisions == 0 {
            doc.hint("add the acus guard PreToolUse hook (acus self setup)");
            return;
        }
        match &self.projects {
            Some(rows) => self.projects_table(doc, rows),
            None => self.rules_table(doc),
        }
        if !self.escaped.is_empty() {
            doc.section("latest escaped calls");
            let rows = self
                .escaped
                .iter()
                .map(|e| vec![e.rule.clone(), e.call.replace('\n', "⏎")])
                .collect();
            doc.table(table(&["rule", "call"], "ll", rows));
        }
        if self.escaped_hidden > 0 {
            doc.more(self.escaped_hidden, "census usage --guard --top N");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: u64, session: &str, rule: &str, kind: &str) -> GuardEntry {
        GuardEntry {
            ts,
            session: Some(session.to_owned()),
            rule: rule.to_owned(),
            kind: kind.to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn a_rule_shows_its_follow_rate_only_after_the_minimum_sample() {
        let mut log = Vec::new();
        for i in 0..5 {
            log.push(line(i * 1_000, &format!("s{i}"), "grep", "hint"));
        }
        log.push(line(100_000, "x", "sed", "escape"));
        let view = GuardView::new("log".to_owned(), &log, 10, false);
        let grep = view.rules.iter().find(|r| r.rule == "grep").expect("grep");
        assert_eq!(grep.observed, 5);
        assert_eq!(grep.follow, Some(1.0));
        assert!(view.projects.is_none());
    }

    #[test]
    fn a_rule_below_the_minimum_sample_has_no_rate() {
        let log = [
            line(0, "s", "grep", "deny"),
            line(10_000, "x", "sed", "escape"),
        ];
        let view = GuardView::new("log".to_owned(), &log, 10, false);
        assert_eq!(view.rules[0].observed, 1);
        assert_eq!(view.rules[0].follow, None);
        assert_eq!(by_rule(&log)["grep"].deny, 1);
    }

    #[test]
    fn per_project_rows_are_truncated_and_name_the_rest() {
        let log: Vec<GuardEntry> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, dir)| GuardEntry {
                cwd: Some(format!("/work/{dir}")),
                rule: "grep".to_owned(),
                kind: "deny".to_owned(),
                ts: i as u64,
                ..Default::default()
            })
            .collect();
        let view = GuardView::new("log".to_owned(), &log, 2, true);
        assert_eq!(view.projects.as_ref().map(Vec::len), Some(2));
        assert_eq!(view.projects_hidden, 1);
    }
}
