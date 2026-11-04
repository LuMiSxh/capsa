// SPDX-License-Identifier: MPL-2.0
//! `acus files`: lists files under a path, with optional line counts.

use crate::fmt::{Budget, Out, clip};
use crate::globs::{add_type_globs, searched_globs};
use acus_walk::{WalkOpts, display_path, walk};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::paths::expand_home;
use capsa_core::render::{Align, Doc, Table};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Lines printed before the rest is cut; the cut line names the way to narrow the listing.
const MAX_FILES: usize = 200;

#[derive(clap::Args)]
pub struct Args {
    /// Files or directories to list (default: the current directory).
    paths: Vec<String>,
    /// Include glob; prefix with ! to exclude (repeatable).
    #[arg(short = 'g', long = "glob")]
    globs: Vec<String>,
    /// Also list hidden files and directories.
    #[arg(long)]
    hidden: bool,
    /// Also list files excluded by .gitignore.
    #[arg(short = 'u', long)]
    no_ignore: bool,
    /// Follow symlinks while walking, as rg -L (grep's -L lists files without a match).
    #[arg(short = 'L', long)]
    follow: bool,
    /// File type like rg: rs, py, ts, js, md, … (repeatable).
    #[arg(short = 't', long = "type")]
    types: Vec<String>,
    /// Append each file's line count (`path  N`, `-` for binary files), then a total line.
    /// Counts newlines, as `wc -l` does.
    #[arg(long)]
    lines: bool,
    /// Do not descend more than N levels below each path (alias --max-depth).
    #[arg(long, visible_alias = "max-depth", value_name = "N")]
    depth: Option<usize>,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each printed line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    // Named in the hint when nothing matches.
    let searched = searched_globs(&a.globs, &a.paths);
    let (exclude, mut include): (Vec<String>, Vec<String>) =
        a.globs.into_iter().partition(|g| g.starts_with('!'));
    add_type_globs(&a.types, &mut include);
    let opts = WalkOpts {
        roots: a
            .paths
            .iter()
            .map(|p| expand_home(p).into())
            .collect::<Vec<PathBuf>>(),
        include,
        // As in `find`: a trailing `/` names a directory; globs match the bare name.
        exclude: exclude
            .iter()
            .map(|g| g[1..].trim_end_matches('/').to_owned())
            .collect(),
        hidden: a.hidden,
        no_ignore: a.no_ignore,
        follow: a.follow,
        max_depth: a.depth,
    };
    let found = Mutex::new(Vec::new());
    walk(&opts, |p| {
        found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(display_path(p));
    })?;
    let mut files = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    files.sort();
    if files.is_empty() {
        if out.format != Format::Json && !(a.hidden && a.no_ignore) {
            eprintln!("(no files; hidden and .gitignored files were skipped: --hidden, -u)");
        }
        if out.format != Format::Json
            && let Some(s) = &searched
        {
            eprintln!("(searched {s})");
        }
        return Ok(Outcome::Empty);
    }
    // The total covers every file, including those the listing cuts off.
    let counts = a.lines.then(|| line_counts(&files));
    let total: usize = counts.iter().flatten().flatten().sum();
    let shown = files.len().min(MAX_FILES);
    let rest = files.len() - shown;
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    let cut = a.cut.map(|n| n as usize);
    if out.format == Format::Json {
        let json = match &counts {
            Some(c) => {
                let rows: Vec<_> = files
                    .iter()
                    .zip(c)
                    .take(shown)
                    .map(|(f, n)| serde_json::json!({ "path": f, "lines": n }))
                    .collect();
                serde_json::json!({ "files": rows, "total": total, "truncated": rest })
            }
            None => serde_json::json!({ "files": &files[..shown], "truncated": rest }),
        };
        outln!("{json}");
    } else if out.format == Format::Human {
        print_human(
            &files[..shown],
            counts.as_deref(),
            total,
            rest,
            out,
            &mut budget,
            cut,
        )?;
    } else {
        for (i, f) in files[..shown].iter().enumerate() {
            let line = match &counts {
                Some(c) => format!("{f}  {}", c[i].map_or("-".to_owned(), |n| n.to_string())),
                None => f.clone(),
            };
            let line = cut.map_or(line.clone(), |n| clip(&line, 0, n));
            budget.emit(&line);
        }
        if rest > 0 {
            budget.emit(&format!("… {rest} more; narrow with -g or a path"));
        }
        if counts.is_some() {
            budget.emit(&format!("total  {total}"));
        }
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
    Ok(Outcome::Found)
}

/// The human view: a table of paths, with line counts and a total under `--lines`, then the
/// rest that the listing cut off.
fn print_human(
    shown: &[String],
    counts: Option<&[Option<usize>]>,
    total: usize,
    rest: usize,
    out: &Out,
    budget: &mut Budget,
    cut: Option<usize>,
) -> std::io::Result<()> {
    let mut head = vec!["path".to_owned()];
    let mut align = vec![Align::Left];
    let clip_cell = |s: &str| cut.map_or_else(|| s.to_owned(), |n| clip(s, 0, n));
    let mut rows: Vec<Vec<String>> = shown.iter().map(|f| vec![clip_cell(f)]).collect();
    if let Some(counts) = counts {
        head.push("lines".to_owned());
        align.push(Align::Right);
        for (row, n) in rows.iter_mut().zip(counts) {
            row.push(n.map_or("-".to_owned(), |n| n.to_string()));
        }
        rows.push(vec!["total".to_owned(), total.to_string()]);
    }
    let mut doc = Doc::default();
    doc.table(Table { head, align, rows });
    if rest > 0 {
        doc.more(rest, "acus files -g GLOB or PATH");
    }
    out.doc_budget(&doc, budget)
}

/// Line counts of `files`, in order, counted on all cores. Workers take the next file from a
/// shared counter, so one big file does not leave the others waiting.
fn line_counts(files: &[String]) -> Vec<Option<usize>> {
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = vec![None; files.len()];
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(f) = files.get(i) else { break };
                        done.push((i, count_lines(f)));
                    }
                    done
                })
            })
            .collect();
        for h in handles {
            for (i, n) in h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)) {
                counts[i] = n;
            }
        }
    });
    counts
}

/// Newlines in `path`, as `wc -l` counts them; `None` for a file with a NUL byte or one that
/// cannot be read.
fn count_lines(path: &str) -> Option<usize> {
    let bytes = std::fs::read(path).ok()?;
    (!bytes.contains(&0)).then(|| bytes.iter().filter(|&&b| b == b'\n').count())
}
