// SPDX-License-Identifier: MPL-2.0
//! `acus show`: prints files, line ranges and symbols, optionally as of a git revision.

use crate::fmt::{Budget, Out, clip, more_lines};
use crate::source::{Span, address, locate, read_text, revision, show_blob};
use acus_syntax::{Outline, Target};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use std::collections::HashMap;

#[derive(clap::Args)]
pub struct Args {
    /// path, path:START-END, path:LINE, path:N- (line N to the end), path:-N (last N lines) or
    /// path#Symbol (a unique suffix of the qualified name is enough). `REV:path` shows the file
    /// as of a git revision (commit, branch or tag): `REV:src/a.rs` counts from the repo root as
    /// git does, `REV:./src/a.rs` from the current directory.
    #[arg(required = true, value_parser = crate::home::<String>)]
    addrs: Vec<String>,
    /// Cap per address.
    #[arg(long, default_value_t = 400)]
    max_lines: usize,
    /// Extra lines around symbol and range targets.
    #[arg(long, default_value_t = 0)]
    context: usize,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each source line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let mut json = Vec::new();
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    // Each file is read and outlined once, even when several addresses name it.
    let mut sources: HashMap<String, (String, String)> = HashMap::new();
    let mut outlines: HashMap<String, Option<Outline>> = HashMap::new();
    let mut shown = 0;
    let mut show = |raw: &String| -> Result<()> {
        let addr = address(raw);
        let path = &addr.path;
        if !sources.contains_key(path) {
            // The language comes from the path after the colon, so `REV:x.rs` is still Rust.
            let (src, file) = match revision(path) {
                Some((rev, file)) => (show_blob(rev, file, path)?, file),
                None => (read_text(path)?, path.as_str()),
            };
            sources.insert(path.clone(), (src, file.to_string()));
        }
        let (src, file) = &sources[path];
        let lines: Vec<&str> = src.lines().collect();
        let Span { label, start, end } = locate(&addr, file, src, &lines, &mut outlines)?;
        // `tail -n` counts exactly N lines and `tail -n +N` starts at line N, so context does not
        // widen either.
        let widen = if matches!(
            addr.target,
            Target::Whole | Target::Tail(_) | Target::From(_)
        ) {
            0
        } else {
            a.context
        };
        let start = start.saturating_sub(widen).max(1);
        let end = (end + widen).min(lines.len());
        let shown_end = end.min(start + a.max_lines.max(1) - 1);
        shown += 1;
        if out.format == Format::Json {
            json.push(serde_json::json!({
                "address": label,
                "start": start,
                "end": shown_end,
                "text": lines.get(start - 1..shown_end).unwrap_or_default().join("\n"),
            }));
            return Ok(());
        }
        budget.emit(&out.header(&format!("== {label} {start}-{end}")));
        for (i, line) in lines.iter().enumerate().take(shown_end).skip(start - 1) {
            let clipped = a.cut.map(|n| clip(line, 0, n as usize));
            budget.emit(&out.numbered(i + 1, clipped.as_deref().unwrap_or(*line), false));
        }
        if shown_end < end {
            budget.emit(&more_lines(path, shown_end + 1, end, end - shown_end));
        }
        Ok(())
    };
    // With several addresses, a failed one is reported and the others are still shown.
    let mut failed = 0;
    for raw in &a.addrs {
        if let Err(e) = show(raw) {
            if a.addrs.len() == 1 {
                return Err(e);
            }
            eprintln!("error: {e:#}");
            failed += 1;
        }
    }
    if let Some(f) = budget.footer() {
        println!("{f}");
    }
    if out.format == Format::Json {
        println!("{}", serde_json::Value::Array(json));
    }
    // Failed addresses were reported above: exit 2 when nothing could be shown, 1 when some were.
    Ok(match (shown, failed) {
        (_, 0) => Outcome::Found,
        (0, _) => Outcome::Code(2),
        _ => Outcome::Code(1),
    })
}
