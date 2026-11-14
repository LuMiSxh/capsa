// SPDX-License-Identifier: MPL-2.0
//! `acus show`: prints files, line ranges and symbols, optionally as of a git revision.

use crate::cmd_outline;
use crate::out::{Budget, Out, clip, fit_line, more_lines};
use crate::source::{Span, address, locate, read_text, revision, show_blob};
use acus_syntax::{Lang, Outline, Target};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

/// Whole files longer than this show their outline and first lines instead of all of them.
const WHOLE_FILE_LINES: usize = 400;
/// Lines of a long whole file shown under the outline.
const WHOLE_FILE_HEAD: usize = 60;
/// Characters printed across all addresses before the rest is named; under Claude Code's cap.
const CHAR_BUDGET: usize = 24_000;

#[derive(clap::Args)]
pub struct Args {
    /// path, path:START-END, path:LINE, path:N- (line N to the end), path:-N (last N lines) or
    /// path#Symbol (a unique suffix of the qualified name is enough). `REV:path` shows the file
    /// as of a git revision (commit, branch or tag): `REV:src/a.rs` counts from the repo root as
    /// git does, `REV:./src/a.rs` from the current directory.
    #[arg(required = true, value_parser = crate::home::<String>)]
    addrs: Vec<String>,
    /// Cap per address; without it a whole file over 400 lines shows its outline and first 60
    /// lines.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    max_lines: Option<u64>,
    /// Extra lines around symbol and range targets.
    #[arg(long, default_value_t = 0)]
    context: usize,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each source line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Print every line: no whole-file cap and no output budget.
    #[arg(long, conflicts_with = "max_lines")]
    full: bool,
}

/// What one printed line did under the two budgets.
enum Emit {
    Printed,
    /// The `--head` budget is spent; the line was counted for its footer.
    Dropped,
    /// The call's character budget is spent; printing stops.
    CharCut,
}

/// Where the character budget stopped.
enum Cut {
    /// The address was cut mid-way: these addresses continue the output, this one first.
    Rest(Vec<String>),
    /// The first line did not fit and was clipped: only `--full` prints it whole.
    FirstLine,
}

/// The `--head` budget plus the call-wide character budget.
struct Sink<'a> {
    budget: &'a mut Budget,
    chars_left: Option<usize>,
}

impl Sink<'_> {
    fn emit(&mut self, line: &str) -> Emit {
        if self.budget.exhausted() {
            self.budget.emit(line);
            return Emit::Dropped;
        }
        if let Some(left) = &mut self.chars_left {
            let n = line.chars().count() + 1;
            if n > *left {
                return Emit::CharCut;
            }
            *left -= n;
        }
        self.budget.emit(line);
        Emit::Printed
    }

    /// Prints `line` clipped to the remaining character budget, with `clip`'s marker. The rest
    /// of the call is then named by `--full`, never by repeating the address.
    fn emit_clipped(&mut self, line: &str) -> bool {
        if self.budget.exhausted() {
            self.budget.emit(line);
            return false;
        }
        let Some(left) = &mut self.chars_left else {
            self.budget.emit(line);
            return true;
        };
        let text = fit_line(line, *left);
        if text.is_empty() {
            return false;
        }
        *left = left.saturating_sub(text.chars().count() + 1);
        self.budget.emit(&text);
        true
    }

    /// Prints a footer under `--head`, outside the character budget.
    fn footer(&mut self, line: &str) {
        self.budget.emit(line);
    }
}

/// One address's lines and the part of them that this call prints.
struct Part<'s> {
    path: &'s str,
    file: &'s str,
    src: &'s str,
    lines: &'s [&'s str],
    start: usize,
    end: usize,
    shown_end: usize,
    long_whole: bool,
}

/// The state of one `show` call: sources read once, and both output budgets.
struct Shown<'a> {
    a: &'a Args,
    out: &'a Out,
    sink: Sink<'a>,
    json: Vec<serde_json::Value>,
    sources: HashMap<String, Rc<(String, String)>>,
    outlines: HashMap<String, Option<Outline>>,
    count: usize,
    cut: Option<Cut>,
}

impl Shown<'_> {
    fn load(&mut self, path: &str) -> Result<()> {
        if self.sources.contains_key(path) {
            return Ok(());
        }
        // The language comes from the path after the colon, so `REV:x.rs` is still Rust.
        let (src, file) = match revision(path) {
            Some((rev, file)) => (show_blob(rev, file, path)?, file),
            None => (read_text(path)?, path),
        };
        self.sources
            .insert(path.to_owned(), Rc::new((src, file.to_owned())));
        Ok(())
    }

    fn show(&mut self, idx: usize, raw: &str) -> Result<()> {
        let addr = address(raw);
        let path = addr.path.clone();
        self.load(&path)?;
        let source = Rc::clone(&self.sources[&path]);
        let (src, file) = (source.0.as_str(), source.1.as_str());
        let lines: Vec<&str> = src.lines().collect();
        let Span { label, start, end } = locate(&addr, file, src, &lines, &mut self.outlines)?;
        let widen = self.widen(&addr.target);
        let start = start.saturating_sub(widen).max(1);
        let end = (end + widen).min(lines.len());
        let long_whole = addr.target == Target::Whole
            && end > WHOLE_FILE_LINES
            && self.a.max_lines.is_none()
            && !self.a.full;
        self.count += 1;
        // JSON is for scripts: the whole file and the call budget never cut it.
        if self.out.format == Format::Json {
            self.json.push(json_entry(&label, start, end, &lines));
            return Ok(());
        }
        let part = Part {
            path: &path,
            file,
            src,
            lines: &lines,
            start,
            end,
            shown_end: shown_end(self.a.max_lines, start, end, long_whole),
            long_whole,
        };
        self.print(idx, &label, &part);
        Ok(())
    }

    /// Returns the context lines added around a target; `tail` and `from` never widen.
    fn widen(&self, target: &Target) -> usize {
        // `tail -n` counts exactly N lines and `tail -n +N` starts at line N, so context does not
        // widen either.
        if matches!(target, Target::Whole | Target::Tail(_) | Target::From(_)) {
            0
        } else {
            self.a.context
        }
    }

    /// Prints one address: its header, the outline of a long whole file, its lines and footers.
    fn print(&mut self, idx: usize, label: &str, part: &Part<'_>) {
        let a: &Args = self.a;
        let header = self
            .out
            .header(&format!("== {label} {}-{}", part.start, part.end));
        if matches!(self.sink.emit(&header), Emit::CharCut) {
            self.cut = Some(Cut::FirstLine);
            return;
        }
        let later = &a.addrs[idx + 1..];
        let mut printed = 0;
        if part.long_whole {
            let block = outline_block(
                &mut self.outlines,
                self.out,
                part.path,
                part.file,
                part.src,
                a.cut.map(|n| n as usize),
            );
            for line in block {
                let rest = || resume_from(part.path, part.start, part.end, later);
                if let Some(cut) = self.emit_row(&line, &mut printed, rest) {
                    self.cut = Some(cut);
                    return;
                }
            }
        }
        for (i, line) in part
            .lines
            .iter()
            .enumerate()
            .take(part.shown_end)
            .skip(part.start - 1)
        {
            let clipped = a.cut.map(|n| clip(line, 0, n as usize));
            let text = self
                .out
                .numbered(i + 1, clipped.as_deref().unwrap_or(*line), false);
            let rest = || resume_from(part.path, i + 1, part.end, later);
            if let Some(cut) = self.emit_row(&text, &mut printed, rest) {
                self.cut = Some(cut);
                return;
            }
        }
        self.footers(part);
    }

    /// Emits one printed row; returns the cut when the character budget ends the call at it.
    fn emit_row(
        &mut self,
        text: &str,
        printed: &mut usize,
        rest: impl FnOnce() -> Vec<String>,
    ) -> Option<Cut> {
        match self.sink.emit(text) {
            Emit::Printed => {
                *printed += 1;
                None
            }
            Emit::Dropped => None,
            Emit::CharCut if *printed == 0 => {
                self.sink.emit_clipped(text);
                Some(Cut::FirstLine)
            }
            Emit::CharCut => Some(Cut::Rest(rest())),
        }
    }

    /// Names the lines that the line budget left out, after the printed lines.
    fn footers(&mut self, part: &Part<'_>) {
        let (path, shown_end, end) = (part.path, part.shown_end, part.end);
        if shown_end >= end {
            return;
        }
        if part.long_whole {
            self.sink.footer(&format!(
                "… {} more lines: acus show {path}:{}-{end}",
                end - shown_end,
                shown_end + 1
            ));
            self.sink.footer(&format!("or acus show {path}#Symbol"));
        } else {
            self.sink
                .footer(&more_lines(path, shown_end + 1, end, end - shown_end));
        }
    }
}

/// Returns the JSON record of one address: its label, its range and the text of that range.
fn json_entry(label: &str, start: usize, end: usize, lines: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "address": label,
        "start": start,
        "end": end,
        "text": lines.get(start - 1..end).unwrap_or_default().join("\n"),
    })
}

/// Returns the last line one address prints: the `--max-lines` cap, the head of a long whole file,
/// or the end of the range.
fn shown_end(max_lines: Option<u64>, start: usize, end: usize, long_whole: bool) -> usize {
    let cap = match max_lines {
        Some(n) => Some(start + n.max(1) as usize - 1),
        None if long_whole => Some(start + WHOLE_FILE_HEAD - 1),
        None => None,
    };
    cap.map_or(end, |c| c.min(end))
}

/// Returns the addresses that continue the output at `path:first-end` after a cut.
fn resume_from(path: &str, first: usize, end: usize, later: &[String]) -> Vec<String> {
    let mut rest = vec![format!("{path}:{first}-{end}")];
    rest.extend(later.iter().cloned());
    rest
}

/// The outline of a long whole file, with `cut` applied to each symbol's text.
fn outline_block(
    outlines: &mut HashMap<String, Option<Outline>>,
    out: &Out,
    path: &str,
    file: &str,
    src: &str,
    cut: Option<usize>,
) -> Vec<String> {
    let o = outlines.entry(path.to_owned()).or_insert_with(|| {
        Lang::from_path(Path::new(file)).and_then(|l| acus_syntax::outline(l, src))
    });
    let folded = matches!(
        o.as_ref().map(|o| o.lang),
        Some(Lang::Toml | Lang::Json | Lang::Yaml)
    );
    let depth = if folded { 1 } else { usize::MAX };
    cmd_outline::render(path, o.as_ref(), depth, folded, cut, out)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    // Each file is read and outlined once, even when several addresses name it.
    let mut shown = Shown {
        a: &a,
        out,
        sink: Sink {
            budget: &mut budget,
            chars_left: (!a.full).then_some(CHAR_BUDGET),
        },
        json: Vec::new(),
        sources: HashMap::new(),
        outlines: HashMap::new(),
        count: 0,
        cut: None,
    };
    // With several addresses, a failed one is reported and the others are still shown.
    let mut failed = 0;
    for (idx, raw) in a.addrs.iter().enumerate() {
        if shown.cut.is_some() {
            break;
        }
        if let Err(e) = shown.show(idx, raw) {
            if a.addrs.len() == 1 {
                return Err(e);
            }
            eprintln!("error: {e:#}");
            failed += 1;
        }
    }
    if let Some(cut) = &shown.cut {
        match cut {
            Cut::Rest(rest) => outln!(
                "… output budget spent ({CHAR_BUDGET} chars); the rest: acus show {}",
                rest.join(" ")
            ),
            Cut::FirstLine => {
                outln!("… output budget spent ({CHAR_BUDGET} chars); rerun with --full");
            }
        }
    }
    if let Some(f) = shown.sink.budget.footer() {
        outln!("{f}");
    }
    if out.format == Format::Json {
        outln!("{}", serde_json::Value::Array(shown.json));
    }
    // Failed addresses were reported above: exit 2 when nothing could be shown, 1 when some were.
    Ok(match (shown.count, failed) {
        (_, 0) => Outcome::Found,
        (0, _) => Outcome::Code(2),
        _ => Outcome::Code(1),
    })
}
