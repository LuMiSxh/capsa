// SPDX-License-Identifier: MPL-2.0
//! `acus files`: lists files under a path, with optional line counts.

use crate::fmt::Out;
use crate::globs::{add_type_globs, searched_globs};
use acus_walk::{WalkOpts, display_path, walk};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use capsa_core::paths::expand_home;
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
        println!("{json}");
    } else {
        for (i, f) in files[..shown].iter().enumerate() {
            match &counts {
                Some(c) => println!("{f}  {}", c[i].map_or("-".to_owned(), |n| n.to_string())),
                None => println!("{f}"),
            }
        }
        if rest > 0 {
            println!("… {rest} more; narrow with -g or a path");
        }
        if counts.is_some() {
            println!("total  {total}");
        }
    }
    Ok(Outcome::Found)
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
