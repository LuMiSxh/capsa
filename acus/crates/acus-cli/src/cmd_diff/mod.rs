// SPDX-License-Identifier: MPL-2.0
//! `acus diff`: changed lines, summarised per enclosing symbol.

mod moves;
mod new_files;

use crate::fmt::{Budget, Out, clip};
use crate::git::git;
use acus_syntax::{Lang, Outline, outline};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use moves::{Move, MoveIndex, find_moves, moved_line};
use std::path::Path;

/// Printed changed lines per file with `-p`; `--full` lifts the cap.
const FILE_LINES: usize = 80;
/// Label for changed lines outside any symbol.
const TOP: &str = "(top level)";
/// The `git diff` flags every view shares.
const DIFF_FLAGS: &[&str] = &[
    "-c",
    "core.quotepath=off",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "-U0",
    "-M",
    "--relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

#[derive(clap::Args)]
pub struct Args {
    /// Optional revision (default HEAD, or `A..B`), then paths to limit to.
    #[arg(value_parser = crate::home::<String>)]
    args: Vec<String>,
    /// Staged changes only (index against the revision).
    #[arg(long, alias = "cached")]
    staged: bool,
    /// Also print the changed lines under each symbol.
    #[arg(short = 'p', long)]
    lines: bool,
    /// Print full bodies, blank lines and every changed line: no move summary, outline or cap.
    #[arg(long)]
    full: bool,
    /// Cap on printed lines.
    #[arg(long, default_value_t = 300)]
    max_lines: usize,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// With -p, clip each changed line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Accepted for git habits: the per-file summary is already the default output.
    #[arg(long = "stat", hide = true)]
    _stat: bool,
}

#[derive(Default)]
struct FileDiff {
    /// None for added files.
    old: Option<String>,
    /// None for deleted files.
    new: Option<String>,
    binary: bool,
    /// Not in the index: shown as `?`, like git's `??`.
    untracked: bool,
    /// (added, line number on that side, text).
    lines: Vec<(bool, usize, String)>,
}

struct Group {
    label: String,
    range: Option<(usize, usize)>,
    add: usize,
    del: usize,
    lines: Vec<(bool, usize, String)>,
    /// The move this group belongs to and whether it is the destination side.
    moved: Option<(usize, bool)>,
}

/// A changed file with the sources and outlines the text view needs beyond its diff lines.
struct View<'a> {
    f: &'a FileDiff,
    old_src: Option<String>,
    new_src: Option<String>,
    old_o: Option<Outline>,
    new_o: Option<Outline>,
    /// The new-side outline as `acus outline` prints it, without attached comments.
    display_o: Option<Outline>,
}

/// `git diff` summarised per enclosing symbol, plus untracked files.
#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    // Like git: the first argument is a revision unless it names an existing path.
    let (rev, paths) = match a.args.split_first() {
        Some((r, rest)) if !Path::new(r).exists() => (Some(r.as_str()), rest),
        _ => (None, &a.args[..]),
    };
    let range = rev.and_then(|r| r.split_once(".."));
    // ponytail: `A...B` compares against A, not the merge base; only the symbol labels of removed
    // lines are affected.
    let old_rev = range.map_or(
        rev.unwrap_or("HEAD"),
        |(l, _)| {
            if l.is_empty() { "HEAD" } else { l }
        },
    );
    let new_rev = range
        .map(|(_, r)| r.trim_start_matches('.'))
        .map(|r| if r.is_empty() { "HEAD" } else { r });
    let files = changed_files(&a, rev, paths, range.is_some())?;
    // The default view is the working tree against the index: staged changes are named in a footer.
    let staged = if out.format != Format::Json && rev.is_none() && !a.staged {
        staged_footer(paths)?
    } else {
        None
    };
    if files.is_empty() {
        if let Some(line) = staged {
            outln!("{}", out.dim(&line));
            return Ok(Outcome::Found);
        }
        if out.format == Format::Json {
            // JSON prints `null` for no changes; the exit code stays 0.
            outln!("null");
        } else {
            eprintln!("(no changes)");
        }
        return Ok(Outcome::Code(0));
    }
    let rendered = render(&a, &files, rev, old_rev, new_rev);
    if out.format == Format::Json {
        print_json_files(&a, &rendered);
        return Ok(Outcome::Found);
    }
    print_text_files(&a, out, &files, &rendered, staged, rev);
    Ok(Outcome::Found)
}

/// The changed files of `rev` (or the working tree), plus untracked files unless the diff is
/// ranged or staged.
fn changed_files(
    a: &Args,
    rev: Option<&str>,
    paths: &[String],
    ranged: bool,
) -> Result<Vec<FileDiff>> {
    let mut cmd = DIFF_FLAGS.to_vec();
    if a.staged {
        cmd.push("--cached");
    }
    cmd.extend(rev);
    cmd.push("--");
    cmd.extend(paths.iter().map(String::as_str));
    let mut files = parse(&git(&cmd).map_err(|e| match rev {
        Some(r) if format!("{e}").contains("revision") => anyhow::Error::new(
            capsa_core::hint::Hinted::new(
                e.to_string(),
                format!("`{r}` is neither an existing path nor a revision; for a deleted file name a revision first: acus diff HEAD {r}"),
            ),
        ),
        _ => e,
    })?);
    if !a.staged && !ranged {
        let mut ls = vec!["ls-files", "--others", "--exclude-standard", "--"];
        ls.extend(paths.iter().map(String::as_str));
        for p in git(&ls)?.lines() {
            let Ok(src) = std::fs::read(p) else { continue };
            let mut f = FileDiff {
                new: Some(p.into()),
                untracked: true,
                ..FileDiff::default()
            };
            if src.contains(&0) {
                f.binary = true;
            } else {
                let src = String::from_utf8_lossy(&src);
                f.lines = src
                    .lines()
                    .enumerate()
                    .map(|(i, l)| (true, i + 1, l.into()))
                    .collect();
            }
            files.push(f);
        }
    }
    Ok(files)
}

/// Each changed file with the sources and outlines the text view needs.
fn render<'a>(
    a: &Args,
    files: &'a [FileDiff],
    rev: Option<&str>,
    old_rev: &str,
    new_rev: Option<&str>,
) -> Vec<View<'a>> {
    // An empty revision reads the index.
    let show = |rev: &str, p: &str| git(&["show", &format!("{rev}:./{p}")]).ok();
    files
        .iter()
        .map(|f| {
            let new_src = match f.new.as_deref() {
                Some(_) if f.old.is_none() => added_source(f),
                Some(p) => match (a.staged, new_rev) {
                    (_, Some(r)) => show(r, p),
                    (true, None) => show("", p),
                    (false, None) => std::fs::read_to_string(p).ok(),
                },
                None => None,
            };
            let old_src = f
                .old
                .as_deref()
                .filter(|_| f.lines.iter().any(|l| !l.0))
                // Unstaged changes are relative to the index, not HEAD.
                .and_then(|p| match (a.staged, rev) {
                    (false, None) => show("", p),
                    _ => show(old_rev, p),
                });
            // Decorators and doc comments count as part of the symbol below them.
            let parse = |p: &Option<String>, src: Option<String>| {
                let p = p.as_deref()?;
                let s = src?;
                Some(outline(Lang::from_path(Path::new(p))?, &s)?.with_attached(&s, p))
            };
            // `acus outline` prints a symbol without its attached comments.
            let display_o = f.new.as_deref().filter(|_| f.old.is_none()).and_then(|p| {
                let src = new_src.as_deref()?;
                outline(Lang::from_path(Path::new(p))?, src)
            });
            let old_o = parse(&f.old, old_src.clone());
            let new_o = parse(&f.new, new_src.clone());
            View {
                f,
                old_src,
                new_src,
                old_o,
                new_o,
                display_o,
            }
        })
        .collect()
}

/// The JSON array: one object per changed file, with its symbol groups.
fn print_json_files(a: &Args, views: &[View<'_>]) {
    let none = MoveIndex::default();
    let v: Vec<_> = views
        .iter()
        .enumerate()
        .map(|(i, view)| {
            let f = view.f;
            let gs = groups(f, view.new_o.as_ref(), view.old_o.as_ref(), i, &none);
            let syms: Vec<_> = gs
                .iter()
                .map(|g| {
                    let mut s = serde_json::json!({
                        "symbol": g.label, "range": g.range, "added": g.add, "removed": g.del,
                    });
                    if a.lines {
                        s["lines"] = g.lines.iter().map(|(add, n, t)| serde_json::json!({
                            "op": if *add { "+" } else { "-" }, "line": n, "text": t,
                        })).collect();
                    }
                    s
                })
                .collect();
            serde_json::json!({
                "status": status(f), "path": f.new.as_ref().or(f.old.as_ref()), "old_path": f.old,
                "binary": f.binary, "added": total(f, true), "removed": total(f, false), "symbols": syms,
            })
        })
        .collect();
    outln!("{}", serde_json::Value::Array(v));
}

/// Output style for the text view.
struct Style<'a> {
    out: &'a Out,
    lines: bool,
    compact: bool,
    cut: Option<usize>,
}

impl Style<'_> {
    fn line(&self, add: bool, n: usize, t: &str) -> String {
        let t = self.cut.map_or_else(|| t.to_owned(), |c| clip(t, 0, c));
        if add {
            self.out.numbered(n, &format!("+{t}"), false)
        } else {
            format!("\t-{t}")
        }
    }
}

/// The text view: one block per file, cut at `--max-lines` and `--head`, then a total and the
/// footer.
fn print_text_files(
    a: &Args,
    out: &Out,
    files: &[FileDiff],
    views: &[View<'_>],
    staged: Option<String>,
    rev: Option<&str>,
) {
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    let style = Style {
        out,
        lines: a.lines,
        compact: a.lines && !a.full,
        cut: a.cut.map(|n| n as usize),
    };
    let moves = if style.compact {
        find_moves(views)
    } else {
        Vec::new()
    };
    let index = MoveIndex::new(&moves);
    let mut printed = 0;
    // The human view lines up the paths and the counts of the file headers.
    let path_w = views
        .iter()
        .map(|v| file_path(v.f).chars().count())
        .max()
        .unwrap_or(0);
    let count_w = views
        .iter()
        .map(|v| counts(total(v.f, true), total(v.f, false)).chars().count())
        .max()
        .unwrap_or(0);
    for (i, view) in views.iter().enumerate() {
        let f = view.f;
        let path = file_path(f);
        let count = counts(total(f, true), total(f, false));
        let head = if out.format == Format::Human {
            let binary = if f.binary { " (binary)" } else { "" };
            format!("{} {path:<path_w$}  {count:>count_w$}{binary}", status(f))
        } else {
            let binary = if f.binary { "(binary)" } else { "" };
            format!("{} {path} {count}{binary}", status(f))
        };
        let mut block = vec![out.header(&head).trim_end().to_owned()];
        let dropped = if style.compact && f.old.is_none() && f.new.is_some() && !f.binary {
            new_files::render(&mut block, &style, view, i, &moves, &index)
        } else {
            text_groups(&mut block, &style, view, i, &moves, &index)
        };
        if dropped > 0 {
            block.push(out.dim(&format!(
                "… {dropped} more changed lines: {}",
                rerun(a, rev, f)
            )));
        }
        if printed > 0 && printed + block.len() > a.max_lines {
            budget.emit(&format!(
                "… {} more files (limit with -- PATH, or drop -p)",
                views.len() - i
            ));
            break;
        }
        printed += block.len();
        block.iter().for_each(|l| budget.emit(l));
    }
    let (add, del) = files.iter().fold((0, 0), |(x, y), f| {
        (x + total(f, true), y + total(f, false))
    });
    budget.emit(&out.header(&format!(
        "== {} file{} {}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        counts(add, del)
    )));
    if let Some(line) = staged {
        budget.emit(&out.dim(&line));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
}

/// Appends the symbol groups of a modified or deleted file, honouring the per-file cap.
/// Returns the changed lines dropped by the cap.
fn text_groups(
    block: &mut Vec<String>,
    style: &Style<'_>,
    view: &View<'_>,
    file: usize,
    moves: &[Move],
    index: &MoveIndex,
) -> usize {
    let mut printed = 0;
    let mut dropped = 0;
    for g in groups(
        view.f,
        view.new_o.as_ref(),
        view.old_o.as_ref(),
        file,
        index,
    ) {
        let header = match g.moved {
            Some((m, true)) => Some(moved_line(&moves[m], &g.label)),
            Some((m, false)) => summary(&g).map(|s| format!("{s} moved → {}", moves[m].to_path)),
            None => summary(&g),
        };
        if let Some(line) = header {
            block.push(line);
        }
        if !style.lines {
            continue;
        }
        let lines: &[(bool, usize, String)] = match g.moved {
            Some((m, true)) => &moves[m].lines,
            Some(_) => &[],
            None => &g.lines,
        };
        for (add, n, t) in lines {
            if style.compact && t.trim().is_empty() {
                continue;
            }
            if style.compact && printed >= FILE_LINES {
                dropped += 1;
                continue;
            }
            block.push(style.line(*add, *n, t));
            printed += 1;
        }
    }
    dropped
}

/// The symbol summary line, or None for whole-file groups.
fn summary(g: &Group) -> Option<String> {
    if g.label.is_empty() {
        return None;
    }
    let range = match g.range {
        Some((s, e)) => format!(" {s}-{e}"),
        None if g.label == TOP => String::new(),
        None => " (removed)".into(),
    };
    Some(format!("  {}{range} {}", g.label, counts(g.add, g.del)))
}

/// The rerun that shows every changed line of a file; `--full` lifts the per-file cap.
fn rerun(a: &Args, rev: Option<&str>, f: &FileDiff) -> String {
    let mut cmd = String::from("acus diff -p --full");
    if let Some(r) = rev {
        cmd.push(' ');
        cmd.push_str(r);
    }
    if a.staged {
        cmd.push_str(" --staged");
    }
    cmd.push_str(" -- ");
    cmd.push_str(f.new.as_deref().or(f.old.as_deref()).unwrap_or_default());
    cmd
}

/// The changed lines of `f`: the added ones when `add`, else the removed ones.
fn total(f: &FileDiff, add: bool) -> usize {
    f.lines.iter().filter(|l| l.0 == add).count()
}

/// The staged changes of the default view as a footer line; None when the index matches HEAD.
fn staged_footer(paths: &[String]) -> Result<Option<String>> {
    let mut cmd = DIFF_FLAGS.to_vec();
    cmd.extend(["--cached", "--"]);
    cmd.extend(paths.iter().map(String::as_str));
    let files = parse(&git(&cmd)?);
    if files.is_empty() {
        return Ok(None);
    }
    let add = files.iter().flat_map(|f| &f.lines).filter(|l| l.0).count();
    let del = files.iter().flat_map(|f| &f.lines).filter(|l| !l.0).count();
    let s = if files.len() == 1 { "" } else { "s" };
    let mut line = format!("staged: {} file{s}", files.len());
    let c = counts(add, del);
    if !c.is_empty() {
        line += &format!(" {c}");
    }
    line += " (acus diff --staged)";
    Ok(Some(line))
}

/// The path a file header names: `old -> new` for a rename.
fn file_path(f: &FileDiff) -> String {
    match (&f.old, &f.new) {
        (Some(o), Some(n)) if o != n => format!("{o} -> {n}"),
        (o, n) => n.clone().or(o.clone()).unwrap_or_default(),
    }
}

fn status(f: &FileDiff) -> &'static str {
    match (&f.old, &f.new) {
        _ if f.untracked => "?",
        (None, _) => "A",
        (_, None) => "D",
        (Some(o), Some(n)) if o != n => "R",
        _ => "M",
    }
}

fn counts(add: usize, del: usize) -> String {
    match (add, del) {
        (0, 0) => String::new(),
        (a, 0) => format!("+{a}"),
        (0, d) => format!("-{d}"),
        (a, d) => format!("+{a} -{d}"),
    }
}

/// Changed lines bucketed by innermost enclosing symbol, in diff order. Whole-file adds and
/// deletes form one unlabelled group; the two sides of a move stay apart.
fn groups(
    f: &FileDiff,
    new: Option<&Outline>,
    old: Option<&Outline>,
    file: usize,
    moves: &MoveIndex,
) -> Vec<Group> {
    let mut v: Vec<Group> = Vec::new();
    let whole = f.old.is_none() || f.new.is_none();
    for (add, n, t) in &f.lines {
        let o = if *add { new } else { old };
        let idx = o.filter(|_| !whole).and_then(|o| enclosing_index(o, *n));
        let sym = idx.and_then(|i| o.and_then(|o| o.symbols.get(i)));
        let label = match sym {
            Some(s) => format!("{} {}", s.kind, s.qual),
            None if whole => String::new(),
            None => TOP.into(),
        };
        let moved = idx.and_then(|i| moves.at(file, *add, i)).map(|m| (m, *add));
        let i = match v.iter().position(|g| g.label == label && g.moved == moved) {
            Some(i) => i,
            None => {
                // Range in the new file, so `acus show path:A-B` works; None if the symbol is gone.
                let range = if matches!(moved, Some((_, false))) {
                    None
                } else {
                    sym.and_then(|s| {
                        new?.symbols
                            .iter()
                            .find(|x| x.qual == s.qual && x.kind == s.kind)
                            .map(|x| (x.start_line, x.end_line))
                    })
                };
                v.push(Group {
                    label,
                    range,
                    add: 0,
                    del: 0,
                    lines: Vec::new(),
                    moved,
                });
                v.len() - 1
            }
        };
        let g = &mut v[i];
        if *add {
            g.add += 1;
        } else {
            g.del += 1;
        }
        g.lines.push((*add, *n, t.clone()));
    }
    v
}

/// The source of an added or untracked file, rebuilt from its added diff lines.
fn added_source(f: &FileDiff) -> Option<String> {
    if f.binary {
        return None;
    }
    let mut src = f
        .lines
        .iter()
        .filter(|(add, _, _)| *add)
        .map(|(_, _, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !src.is_empty() {
        src.push('\n');
    }
    Some(src)
}

fn enclosing_index(o: &Outline, line: usize) -> Option<usize> {
    o.symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.start_line <= line && line <= s.end_line)
        .max_by_key(|(_, s)| s.depth)
        .map(|(i, _)| i)
}

/// Parses `git diff -U0` with `a/` and `b/` prefixes.
fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut o, mut n, mut in_hunk) = (0, 0, false);
    for l in text.lines() {
        if let Some(h) = l.strip_prefix("diff --git a/") {
            let (a, b) = h.split_once(" b/").unwrap_or((h, h));
            files.push(FileDiff {
                old: Some(a.into()),
                new: Some(b.into()),
                ..FileDiff::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if in_hunk {
            if let Some(t) = l.strip_prefix('-') {
                f.lines.push((false, o, t.into()));
                o += 1;
            } else if let Some(t) = l.strip_prefix('+') {
                f.lines.push((true, n, t.into()));
                n += 1;
            }
        }
        if let Some(h) = l.strip_prefix("@@ -") {
            in_hunk = true;
            let num = |s: &str| {
                s.split(',')
                    .next()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0)
            };
            let mut it = h.split(' ');
            o = num(it.next().unwrap_or(""));
            n = num(it.next().unwrap_or("").trim_start_matches('+'));
        } else if in_hunk {
        } else if l.starts_with("new file mode") {
            f.old = None;
        } else if l.starts_with("deleted file mode") {
            f.new = None;
        } else if let Some(p) = l.strip_prefix("rename from ") {
            f.old = Some(p.into());
        } else if let Some(p) = l.strip_prefix("rename to ") {
            f.new = Some(p.into());
        } else if let Some(p) = l.strip_prefix("--- a/") {
            f.old = Some(p.trim_end_matches('\t').into());
        } else if let Some(p) = l.strip_prefix("+++ b/") {
            f.new = Some(p.trim_end_matches('\t').into());
        } else if l.starts_with("Binary files ") {
            f.binary = true;
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_hunks_renames_and_headers_inside_hunks() {
        let d = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -3,2 +3 @@ fn f\n--- dashes\n-b\n+c\n@@ -9,0 +10,2 @@\n+d\n++e\n\
                 diff --git a/o.rs b/n.rs\nsimilarity index 100%\nrename from o.rs\nrename to n.rs\n\
                 diff --git a/new.md b/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/new.md\n@@ -0,0 +1 @@\n+hi\n";
        let f = parse(d);
        assert_eq!(f.len(), 3);
        let l: Vec<_> = f[0]
            .lines
            .iter()
            .map(|(a, n, t)| (*a, *n, t.as_str()))
            .collect();
        assert_eq!(
            l,
            [
                (false, 3, "-- dashes"),
                (false, 4, "b"),
                (true, 3, "c"),
                (true, 10, "d"),
                (true, 11, "+e")
            ]
        );
        assert_eq!(
            (f[1].old.as_deref(), f[1].new.as_deref()),
            (Some("o.rs"), Some("n.rs"))
        );
        assert_eq!((f[2].old.as_deref(), f[2].lines.len()), (None, 1));
    }
}
