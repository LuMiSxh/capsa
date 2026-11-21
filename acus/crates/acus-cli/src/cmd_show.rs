// SPDX-License-Identifier: MPL-2.0
//! `acus show`: prints files, line ranges and symbols, optionally as of a git revision.

use crate::cmd_outline;
use crate::out::{Out, clip, fit_line, more_lines};
use crate::source::{Span, address, locate, read_text, revision, show_blob};
use acus_syntax::{Lang, Outline, Target};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use serde::Serialize;
use serde_json::json;
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

/// One address shown whole: the entry `--json` lists, and what `show()` returns to a script.
#[derive(Debug, Serialize)]
pub(crate) struct ShowEntry {
    address: String,
    pub(crate) path: String,
    /// The qualified name of a `path#Sym` address.
    pub(crate) symbol: Option<String>,
    /// 1-based, inclusive.
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

/// One line of the text output, in print order.
#[derive(Debug, Serialize)]
enum Row {
    /// A file or symbol header, which print colours.
    Header(String),
    Line(String),
    /// A failed address when several were given; print writes it to stderr.
    Error(String),
}

impl Row {
    fn text(&self) -> &str {
        match self {
            Row::Header(text) | Row::Line(text) | Row::Error(text) => text.as_str(),
        }
    }
}

/// Where the character budget ended the call.
#[derive(Debug, Serialize)]
enum Cut {
    /// The address was cut mid-way: these addresses continue the output, this one first.
    Rest(Vec<String>),
    /// The first line did not fit and was clipped: only `--full` prints it whole.
    FirstLine,
}

/// The lines that `--head` dropped, and the cap they were dropped under.
#[derive(Debug, Serialize)]
struct HeadCut {
    head: usize,
    dropped: usize,
}

/// What `show` computed for its addresses: the entries, and the rows that the text output prints.
#[derive(Debug, Serialize)]
pub(crate) struct ShowReport {
    pub(crate) entries: Vec<ShowEntry>,
    rows: Vec<Row>,
    cut: Option<Cut>,
    head_cut: Option<HeadCut>,
    failed: usize,
}

impl ShowReport {
    /// Returns exit 0 when no address failed, 2 when every address failed, and 1 otherwise.
    pub(crate) fn outcome(&self) -> Outcome {
        // Failed addresses were reported above: exit 2 when nothing could be shown, 1 when some were.
        match (self.entries.len(), self.failed) {
            (_, 0) => Outcome::Found,
            (0, _) => Outcome::Code(2),
            _ => Outcome::Code(1),
        }
    }
}

/// What one row did under the two budgets.
enum Emit {
    Printed,
    /// The `--head` budget is spent; the row was counted for its footer.
    Dropped,
    /// The call's character budget is spent; the row goes back to the caller.
    CharCut(Row),
}

/// The text rows of one call, under the `--head` budget and the call's character budget.
struct Rows {
    head: Option<usize>,
    /// Characters left in the call's budget; `None` under `--full`.
    chars_left: Option<usize>,
    printed: usize,
    dropped: usize,
    list: Vec<Row>,
}

impl Rows {
    fn exhausted(&self) -> bool {
        self.head.is_some_and(|n| self.printed >= n)
    }

    fn emit(&mut self, row: Row) -> Emit {
        if self.exhausted() {
            self.dropped += 1;
            return Emit::Dropped;
        }
        if let Some(left) = &mut self.chars_left {
            let n = row.text().chars().count() + 1;
            if n > *left {
                return Emit::CharCut(row);
            }
            *left -= n;
        }
        self.keep(row);
        Emit::Printed
    }

    /// Keeps `text` clipped to the character budget, with `clip`'s marker; false when none fits.
    fn emit_clipped(&mut self, text: &str) -> bool {
        if self.exhausted() {
            self.dropped += 1;
            return false;
        }
        let Some(left) = &mut self.chars_left else {
            self.keep(Row::Line(text.to_owned()));
            return true;
        };
        let fitted = fit_line(text, *left);
        if fitted.is_empty() {
            return false;
        }
        *left = left.saturating_sub(fitted.chars().count() + 1);
        self.keep(Row::Line(fitted));
        true
    }

    /// Keeps a footer line; the character budget does not count footers.
    fn footer(&mut self, text: String) {
        if self.exhausted() {
            self.dropped += 1;
        } else {
            self.keep(Row::Line(text));
        }
    }

    fn keep(&mut self, row: Row) {
        self.printed += 1;
        self.list.push(row);
    }
}

/// The state of one `show` call: sources read once, the entries and the text rows.
struct Walk<'a> {
    a: &'a Args,
    /// Whether the text rows are laid out; the structured format takes whole ranges.
    text: bool,
    /// Formats without colour; print colours the header rows.
    plain: Out,
    sources: HashMap<String, Rc<(String, String)>>,
    outlines: HashMap<String, Option<Outline>>,
    entries: Vec<ShowEntry>,
    rows: Rows,
    cut: Option<Cut>,
}

/// One address's lines and the part of them that its text rows print.
struct Part<'s> {
    label: &'s str,
    path: &'s str,
    file: &'s str,
    src: &'s str,
    lines: &'s [&'s str],
    start: usize,
    end: usize,
    shown_end: usize,
    long_whole: bool,
}

impl<'a> Walk<'a> {
    fn new(a: &'a Args, text: bool) -> Self {
        Walk {
            a,
            text,
            plain: Out {
                format: Format::Agent,
                color: false,
            },
            sources: HashMap::new(),
            outlines: HashMap::new(),
            entries: Vec::new(),
            rows: Rows {
                head: a.head.map(|n| n as usize),
                chars_left: (!a.full).then_some(CHAR_BUDGET),
                printed: 0,
                dropped: 0,
                list: Vec::new(),
            },
            cut: None,
        }
    }

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
        let symbol = label
            .strip_prefix(path.as_str())
            .and_then(|rest| rest.strip_prefix('#'))
            .map(str::to_owned);
        if self.text {
            let part = Part {
                label: &label,
                path: &path,
                file,
                src,
                lines: &lines,
                start,
                end,
                shown_end: shown_end(self.a.max_lines, start, end, long_whole),
                long_whole,
            };
            self.layout(idx, &part);
        }
        self.entries.push(ShowEntry {
            address: label,
            path,
            symbol,
            start,
            end,
            text: lines.get(start - 1..end).unwrap_or_default().join("\n"),
        });
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

    /// Lays out one address: its header, the outline of a long whole file, its lines and footers.
    fn layout(&mut self, idx: usize, part: &Part<'_>) {
        let a = self.a;
        let header = Row::Header(format!("== {} {}-{}", part.label, part.start, part.end));
        if let Emit::CharCut(_) = self.rows.emit(header) {
            self.cut = Some(Cut::FirstLine);
            return;
        }
        let later = &a.addrs[idx + 1..];
        let mut printed = 0;
        if part.long_whole {
            for row in self.outline_rows(part) {
                let rest = || resume_from(part.path, part.start, part.end, later);
                if let Some(cut) = self.emit_row(row, &mut printed, rest) {
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
                .plain
                .numbered(i + 1, clipped.as_deref().unwrap_or(*line), false);
            let rest = || resume_from(part.path, i + 1, part.end, later);
            if let Some(cut) = self.emit_row(Row::Line(text), &mut printed, rest) {
                self.cut = Some(cut);
                return;
            }
        }
        self.footers(part);
    }

    /// Returns the outline rows of a long whole file, with `--cut` applied to each symbol.
    fn outline_rows(&mut self, part: &Part<'_>) -> Vec<Row> {
        let cut = self.a.cut.map(|n| n as usize);
        let o = self
            .outlines
            .entry(part.path.to_owned())
            .or_insert_with(|| {
                Lang::from_path(Path::new(part.file))
                    .and_then(|l| acus_syntax::outline(l, part.src))
            });
        let folded = matches!(
            o.as_ref().map(|o| o.lang),
            Some(Lang::Toml | Lang::Json | Lang::Yaml)
        );
        let depth = if folded { 1 } else { usize::MAX };
        let mut lines =
            cmd_outline::render(part.path, o.as_ref(), depth, folded, cut, &self.plain).into_iter();
        // The first line of an outline is its header; without one, render returns a single note.
        let first = lines.next().map(|l| {
            if o.is_some() {
                Row::Header(l)
            } else {
                Row::Line(l)
            }
        });
        first.into_iter().chain(lines.map(Row::Line)).collect()
    }

    /// Lays out one line of an address; returns the cut when the character budget ends the call.
    fn emit_row(
        &mut self,
        row: Row,
        printed: &mut usize,
        rest: impl FnOnce() -> Vec<String>,
    ) -> Option<Cut> {
        match self.rows.emit(row) {
            Emit::Printed => {
                *printed += 1;
                None
            }
            Emit::Dropped => None,
            Emit::CharCut(row) if *printed == 0 => {
                self.rows.emit_clipped(row.text());
                Some(Cut::FirstLine)
            }
            Emit::CharCut(_) => Some(Cut::Rest(rest())),
        }
    }

    /// Names the lines that the line budget left out, after the printed lines.
    fn footers(&mut self, part: &Part<'_>) {
        let (path, shown_end, end) = (part.path, part.shown_end, part.end);
        if shown_end >= end {
            return;
        }
        if part.long_whole {
            self.rows.footer(format!(
                "… {} more lines: acus show {path}:{}-{end}",
                end - shown_end,
                shown_end + 1
            ));
            self.rows.footer(format!("or acus show {path}#Symbol"));
        } else {
            self.rows
                .footer(more_lines(path, shown_end + 1, end, end - shown_end));
        }
    }

    fn into_report(self, failed: usize) -> ShowReport {
        let head_cut = self
            .rows
            .head
            .filter(|_| self.rows.dropped > 0)
            .map(|head| HeadCut {
                head,
                dropped: self.rows.dropped,
            });
        ShowReport {
            entries: self.entries,
            rows: self.rows.list,
            cut: self.cut,
            head_cut,
            failed,
        }
    }
}

/// Computes what `show` prints for `a`'s addresses, without printing it.
///
/// `Format::Json` takes every address whole, as the structured output does: the caps only shape
/// the text rows. A single address that fails is an error; several are reported as rows.
pub(crate) fn compute(a: &Args, format: Format) -> Result<ShowReport> {
    let mut walk = Walk::new(a, format != Format::Json);
    let mut failed = 0;
    for (idx, raw) in a.addrs.iter().enumerate() {
        if walk.cut.is_some() {
            break;
        }
        if let Err(e) = walk.show(idx, raw) {
            if a.addrs.len() == 1 {
                return Err(e);
            }
            walk.rows.list.push(Row::Error(format!("{e:#}")));
            failed += 1;
        }
    }
    Ok(walk.into_report(failed))
}

/// Prints a `ShowReport`: its text rows and footers, or the JSON array of its entries.
pub(crate) fn print(report: &ShowReport, out: &Out) {
    for row in &report.rows {
        match row {
            Row::Header(text) => outln!("{}", out.header(text)),
            Row::Line(text) => outln!("{text}"),
            Row::Error(text) => eprintln!("error: {text}"),
        }
    }
    match &report.cut {
        Some(Cut::Rest(rest)) => outln!(
            "… output budget spent ({CHAR_BUDGET} chars); the rest: acus show {}",
            rest.join(" ")
        ),
        Some(Cut::FirstLine) => {
            outln!("… output budget spent ({CHAR_BUDGET} chars); rerun with --full");
        }
        None => {}
    }
    if let Some(HeadCut { head, dropped }) = report.head_cut {
        outln!(
            "… output cut at --head {head}; {dropped} more lines (rerun with --head {})",
            head + dropped
        );
    }
    if out.format == Format::Json {
        let entries: Vec<_> = report
            .entries
            .iter()
            .map(|e| json!({"address": e.address, "start": e.start, "end": e.end, "text": e.text}))
            .collect();
        outln!("{}", serde_json::Value::Array(entries));
    }
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let report = compute(&a, out.format)?;
    print(&report, out);
    Ok(report.outcome())
}
