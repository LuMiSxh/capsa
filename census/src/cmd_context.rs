// SPDX-License-Identifier: MPL-2.0
//! `census context`: the context size of the current session against its compaction threshold.

use crate::out::Out;
use crate::usage_views::k;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::hint::ResultExt;
use census::context::{self, ContextUse};
use census::session::{self, Picked};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    /// Session id. Defaults to `CLAUDE_SESSION_ID`, else the most recent session of this directory.
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

pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let resolved = session::resolve_here(a.session.as_deref()).hinted()?;
    let Some((model, tokens)) = context::last_context(&resolved.transcript) else {
        println!("no API request in session {} yet", resolved.id);
        return Ok(Outcome::Empty);
    };
    let c = context::measure(tokens, Some(&model));
    if out.format == Format::Json {
        let body = ContextOut {
            session: &resolved.id,
            picked: resolved.picked,
            context: &c,
        };
        println!("{}", serde_json::to_string(&body)?);
        return Ok(Outcome::Found);
    }
    let context = format!(
        "{} of {} ({:.0}%), {} left",
        k(c.tokens as f64),
        k(c.threshold.tokens as f64),
        c.percent,
        k(c.left as f64)
    );
    println!("{}", out.field("context", &context));
    let threshold = format!(
        "{} from {}",
        k(c.threshold.tokens as f64),
        c.threshold.source
    );
    println!("{}", out.field("threshold", &threshold));
    println!("{}", out.field("model", &model));
    let session = format!("{} ({})", resolved.id, resolved.picked.label());
    println!("{}", out.field("session", &session));
    Ok(Outcome::Found)
}
