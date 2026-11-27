// SPDX-License-Identifier: MPL-2.0
//! The `census usage --output` view: output tokens split into thinking, visible text and tool JSON.

use super::{k, table};
use capsa_core::render::{Doc, Render};
use census::usage::Report;
use serde::Serialize;

/// One row of the output anatomy, in tokens.
#[derive(Serialize)]
struct RoleOutput {
    role: String,
    requests: u64,
    output: u64,
    thinking: u64,
    /// Visible text and tool-call input at four characters per token.
    text: u64,
    tool: u64,
    /// What the three buckets leave of `output`.
    residual: i64,
}

/// `census usage --output`: the output of every role, and of the whole report, split into what the
/// model thought, what it wrote and what it sent as tool-call JSON.
#[derive(Serialize)]
pub(crate) struct OutputView {
    total: RoleOutput,
    roles: Vec<RoleOutput>,
}

impl OutputView {
    pub(crate) fn new(r: &Report) -> Self {
        let roles: Vec<RoleOutput> = r
            .agents
            .iter()
            .map(|a| RoleOutput {
                role: a.role.clone(),
                requests: a.tokens.requests,
                output: a.tokens.output,
                thinking: a.thinking,
                text: a.text_chars / 4,
                tool: a.tool_chars / 4,
                residual: a.tokens.output as i64
                    - a.thinking as i64
                    - (a.text_chars / 4) as i64
                    - (a.tool_chars / 4) as i64,
            })
            .collect();
        Self {
            total: RoleOutput {
                role: "all".into(),
                requests: r.output.requests,
                output: r.output.output,
                thinking: r.output.thinking,
                text: r.output.text_tokens(),
                tool: r.output.tool_tokens(),
                residual: r.output.residual(),
            },
            roles,
        }
    }

    fn row(out: &RoleOutput) -> Vec<String> {
        vec![
            out.role.clone(),
            out.requests.to_string(),
            k(out.output as f64),
            k(out.thinking as f64),
            share(out.thinking as i64, out.output),
            k(out.text as f64),
            share(out.text as i64, out.output),
            k(out.tool as f64),
            share(out.tool as i64, out.output),
            k(out.residual as f64),
            share(out.residual, out.output),
        ]
    }
}

impl Render for OutputView {
    fn doc(&self, doc: &mut Doc) {
        doc.title("census usage --output");
        let t = &self.total;
        doc.row("output").text(format!(
            "{} tokens: thinking {}, visible text {}, tool JSON {}, residual {}",
            k(t.output as f64),
            share(t.thinking as i64, t.output),
            share(t.text as i64, t.output),
            share(t.tool as i64, t.output),
            share(t.residual, t.output)
        ));
        let mut rows: Vec<Vec<String>> = self.roles.iter().map(Self::row).collect();
        rows.push(Self::row(t));
        doc.table(table(
            &[
                "role",
                "requests",
                "output",
                "thinking",
                "thinking%",
                "text",
                "text%",
                "tool JSON",
                "tool%",
                "residual",
                "residual%",
            ],
            "lrrrrrrrrrr",
            rows,
        ));
        doc.line(
            "text and tool JSON are characters / 4; residual is output minus thinking minus both, \
             mostly tokenizer overhead and reasoning a provider does not report",
        );
    }
}

/// `n` as a single-digit share of `total`, or `n/a` without a total.
fn share(n: i64, total: u64) -> String {
    if total == 0 {
        return "n/a".into();
    }
    format!("{:.1}%", 100.0 * n as f64 / total as f64)
}
