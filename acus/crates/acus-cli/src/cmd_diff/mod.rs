// SPDX-License-Identifier: MPL-2.0
//! `acus diff`: changed lines, summarised per enclosing symbol.

mod modes;
mod moves;
mod new_files;
mod raw;
mod render;
mod status;

use crate::fmt::Out;
use crate::git::git;
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use raw::parse;
use status::Status;
use std::path::Path;

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
    /// One `path +A -R` line per file and a total, without symbol groups.
    #[arg(long)]
    stat: bool,
    /// Changed paths only, one per line with the status letter.
    #[arg(long)]
    name_only: bool,
    /// Unmerged paths with their conflict line ranges; exits 1 when there are none.
    #[arg(long)]
    conflicts: bool,
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
    /// Unmerged: shown as `U`, like git's `UU`.
    unmerged: bool,
    /// (added, line number on that side, text).
    lines: Vec<(bool, usize, String)>,
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
    // One `git status` feeds the header, the untracked list and the unmerged paths.
    let status = if rev.is_none() || a.conflicts {
        Some(Status::load(paths)?)
    } else {
        None
    };
    let header = status.as_ref().filter(|_| rev.is_none());
    if a.conflicts {
        let Some(st) = status.as_ref() else {
            return Ok(Outcome::Empty);
        };
        return Ok(modes::conflicts(out, header, st));
    }
    let files = changed_files(&a, rev, paths, range.is_some(), status.as_ref())?;
    // `--stat` and `--name-only` need no outlines or symbol groups.
    let views = if a.stat || a.name_only {
        Vec::new()
    } else {
        render::views(&a, &files, rev, old_rev, new_rev)
    };
    // The default view is the working tree against the index: staged changes are named in a footer.
    let staged =
        if out.format != Format::Json && rev.is_none() && !a.staged && !a.stat && !a.name_only {
            staged_footer(paths)?
        } else {
            None
        };
    if files.is_empty() {
        if out.format == Format::Json {
            render::print_json(&a, header, &files, &views);
            return Ok(Outcome::Found);
        }
        if let Some(h) = header {
            outln!("{}", out.header(&h.header()));
        }
        if let Some(line) = staged {
            outln!("{}", out.dim(&line));
            return Ok(Outcome::Found);
        }
        eprintln!("(no changes)");
        return Ok(Outcome::Code(0));
    }
    if out.format == Format::Json {
        render::print_json(&a, header, &files, &views);
        return Ok(Outcome::Found);
    }
    if a.stat {
        modes::print_stat(&a, out, header, &files);
        return Ok(Outcome::Found);
    }
    if a.name_only {
        modes::print_name_only(&a, out, header, &files);
        return Ok(Outcome::Found);
    }
    render::print_text_files(&a, out, header, &files, &views, staged, rev);
    Ok(Outcome::Found)
}

/// The changed files of `rev` (or the working tree), plus untracked files unless the diff is
/// ranged or staged.
fn changed_files(
    a: &Args,
    rev: Option<&str>,
    paths: &[String],
    ranged: bool,
    status: Option<&Status>,
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
    if let Some(s) = status {
        if !a.staged && !ranged {
            for p in &s.untracked {
                let Ok(src) = std::fs::read(p) else { continue };
                let mut f = FileDiff {
                    new: Some(p.clone()),
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
        // An unmerged file has no ordinary diff, so `git status` names it instead.
        for p in &s.unmerged {
            if files.iter().any(|f| f.new.as_deref() == Some(p.as_str())) {
                continue;
            }
            files.push(FileDiff {
                old: Some(p.clone()),
                new: Some(p.clone()),
                unmerged: true,
                ..FileDiff::default()
            });
        }
    }
    Ok(files)
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
        _ if f.unmerged => "U",
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

#[cfg(test)]
mod tests {
    use super::raw::parse;

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
