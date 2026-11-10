// SPDX-License-Identifier: MPL-2.0
//! `census context`: the context size of the current session against its compaction threshold.

use crate::out::Out;
use crate::usage_views::k;
use anyhow::Result;
use capsa_core::exit::Outcome;

use capsa_core::hint::ResultExt;
use capsa_core::render::{Doc, Render, emit};
use census::context::{self, ContextUse};
use census::session::{self, Picked};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    /// Session id. Defaults to `CLAUDE_CODE_SESSION_ID`, then `CLAUDE_SESSION_ID`, else the most
    /// recent session of this directory.
    #[arg(long, value_name = "ID")]
    session: Option<String>,
}

#[derive(Serialize)]
struct ContextOut<'a> {
    session: &'a str,
    picked: Picked,
    #[serde(flatten)]
    context: &'a ContextUse,
}

impl Render for ContextOut<'_> {
    fn doc(&self, doc: &mut Doc) {
        let c = self.context;
        doc.title("census context");
        doc.row("context").bar(c.percent / 100.0).text(format!(
            "{} of {} ({:.0}%), {} left",
            k(c.tokens as f64),
            k(c.threshold.tokens as f64),
            c.percent,
            k(c.left as f64)
        ));
        doc.row("threshold").text(format!(
            "{} from {}",
            k(c.threshold.tokens as f64),
            c.threshold.source
        ));
        doc.row("model")
            .text(c.model.as_deref().unwrap_or("unknown").to_owned());
        doc.row("session")
            .text(format!("{} ({})", self.session, self.picked.label()));
    }
}

pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let resolved = session::resolve_here(a.session.as_deref()).hinted()?;
    let Some((model, tokens)) = context::last_context(&resolved.transcript) else {
        println!("no API request in session {} yet", resolved.id);
        return Ok(Outcome::Empty);
    };
    let c = context::measure(tokens, Some(&model));
    let body = ContextOut {
        session: &resolved.id,
        picked: resolved.picked,
        context: &c,
    };
    emit(&body, out.format)
}
