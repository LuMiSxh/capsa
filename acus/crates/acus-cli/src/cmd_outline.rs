// SPDX-License-Identifier: MPL-2.0
//! `acus outline`: the symbols of files and directories, nested and filtered by kind.

use crate::fmt::{Budget, Out, clip};
use acus_syntax::{KINDS, Lang, Outline, outline};
use acus_walk::{WalkOpts, display_path, walk};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(clap::Args)]
pub struct Args {
    /// Files or directories (directories are walked).
    #[arg(default_value = ".", value_parser = crate::home::<PathBuf>)]
    paths: Vec<PathBuf>,
    /// Only symbols nested less than N deep (1 = top level).
    #[arg(long)]
    depth: Option<usize>,
    /// Only symbols of these kinds, e.g. `fn,struct` (comma-separated or repeated).
    /// Parents of a match stay as context lines.
    #[arg(short = 'k', long = "kind", value_delimiter = ',')]
    kinds: Vec<String>,
    /// Include glob for directories; prefix with ! to exclude (repeatable).
    #[arg(short = 'g', long = "glob")]
    globs: Vec<String>,
    #[arg(long)]
    hidden: bool,
    /// Cap on printed lines.
    #[arg(long, default_value_t = 400)]
    max_lines: usize,
    /// Stop after N printed lines, headers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each symbol's text to N characters; a clipped text ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
}

/// Names the first path that does not exist, with the hint `acus find` gives for it.
fn check_paths(paths: &[PathBuf]) -> Result<()> {
    match paths.iter().find(|p| !p.exists()) {
        Some(p) => Err(crate::source::not_found(&p.to_string_lossy())),
        None => Ok(()),
    }
}

pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    check_kinds(&a.kinds)?;
    check_paths(&a.paths)?;
    let (exclude, include): (Vec<String>, Vec<String>) =
        a.globs.into_iter().partition(|g| g.starts_with('!'));
    let found = Mutex::new(Vec::new());
    let parse = |p: &Path| {
        let o = Lang::from_path(p)
            .zip(std::fs::read_to_string(p).ok())
            .and_then(|(l, src)| outline(l, &src));
        (display_path(p), o)
    };
    for p in &a.paths {
        if p.is_file() {
            // Named explicitly: report it even without syntax support.
            found
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(parse(p));
            continue;
        }
        let opts = WalkOpts {
            roots: vec![p.clone()],
            include: include.clone(),
            exclude: exclude.iter().map(|g| g[1..].to_owned()).collect(),
            hidden: a.hidden,
            no_ignore: false,
            ..Default::default()
        };
        walk(&opts, |f| {
            if Lang::from_path(f).is_some() {
                // Parse before locking, or the threads take turns.
                let r = parse(f);
                found
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(r);
            }
        })?;
    }
    let mut files = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !a.kinds.is_empty() {
        for (_, o) in &mut files {
            if let Some(o) = o {
                keep_kinds(o, &a.kinds);
            }
        }
        // Files without a match are left out, so the listing shows only matches.
        files.retain(|(_, o)| o.as_ref().is_some_and(|o| !o.symbols.is_empty()));
    }
    if files.is_empty() {
        return Ok(Outcome::Empty);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let depth = a.depth.unwrap_or(usize::MAX);
    // Data files can hold thousands of nested keys; list the top level unless asked.
    let depth_of = |o: &Option<Outline>| match (a.depth, o.as_ref().map(|o| o.lang)) {
        (None, Some(Lang::Toml | Lang::Json | Lang::Yaml)) => 1,
        _ => depth,
    };
    if out.format == Format::Json {
        let v: Vec<_> = files
            .iter()
            .map(|(path, o)| {
                let syms: Vec<_> = o
                    .iter()
                    .flat_map(|o| &o.symbols)
                    .filter(|s| s.depth < depth_of(o))
                    .collect();
                serde_json::json!({
                    "path": path,
                    "lang": o.as_ref().map(|o| o.lang),
                    "lines": o.as_ref().map(|o| o.lines),
                    "symbols": syms,
                })
            })
            .collect();
        outln!("{}", serde_json::Value::Array(v));
        return Ok(Outcome::Found);
    }
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    let cut = a.cut.map(|n| n as usize);
    let mut printed = 0;
    for (i, (path, o)) in files.iter().enumerate() {
        let folded = a.depth.is_none() && depth_of(o) == 1;
        let block = render(path, o.as_ref(), depth_of(o), folded, cut, out);
        if printed > 0 && printed + block.len() > a.max_lines {
            budget.emit(&format!(
                "… {} more files (narrow with -g, a subdirectory, or --depth 1)",
                files.len() - i
            ));
            break;
        }
        printed += block.len();
        for l in block {
            budget.emit(&l);
        }
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
    Ok(Outcome::Found)
}

pub(crate) fn render(
    path: &str,
    o: Option<&Outline>,
    depth: usize,
    folded: bool,
    cut: Option<usize>,
    out: &Out,
) -> Vec<String> {
    let Some(o) = o else {
        return vec![format!("{path} (no outline)")];
    };
    let mut v = vec![out.header(&format!("{path} {} {}", o.lang.name(), o.lines))];
    let hidden = o.symbols.iter().filter(|s| s.depth >= depth).count();
    v.extend(o.symbols.iter().filter(|s| s.depth < depth).map(|s| {
        let text = s.detail.as_deref().unwrap_or(&s.name);
        let text = cut.map_or_else(|| text.to_owned(), |n| clip(text, 0, n));
        format!(
            "{}{} {} {}-{}",
            "  ".repeat(s.depth),
            s.kind,
            text,
            s.start_line,
            s.end_line
        )
    }));
    if hidden > 0 && folded {
        v.push(format!(
            "  … {hidden} nested (--depth N, or acus show {path}#key::sub)"
        ));
    }
    v
}

fn check_kinds(kinds: &[String]) -> Result<()> {
    match kinds.iter().find(|k| !KINDS.contains(&k.as_str())) {
        Some(k) => Err(capsa_core::hint::Hinted::new(
            format!("unknown kind `{k}`"),
            format!("valid kinds are {}", KINDS.join(", ")),
        )
        .into()),
        None => Ok(()),
    }
}

/// Keeps the symbols of `kinds` and the ancestors that lead to them, so a match still shows
/// where it sits. Ancestors print as context lines, with their indentation.
fn keep_kinds(o: &mut Outline, kinds: &[String]) {
    let symbols = &o.symbols;
    let mut keep = vec![false; symbols.len()];
    for (i, s) in symbols.iter().enumerate() {
        if !kinds.iter().any(|k| k == s.kind) {
            continue;
        }
        keep[i] = true;
        let mut need = s.depth;
        for j in (0..i).rev() {
            if need == 0 {
                break;
            }
            if symbols[j].depth < need {
                keep[j] = true;
                need = symbols[j].depth;
            }
        }
    }
    let mut keep = keep.into_iter();
    o.symbols.retain(|_| keep.next().unwrap_or(false));
}
