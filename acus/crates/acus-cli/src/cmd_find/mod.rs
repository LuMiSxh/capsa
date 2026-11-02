// SPDX-License-Identifier: MPL-2.0
//! `acus find`: regex search, with the enclosing symbol of each hit.

mod print;

use crate::fmt::{Budget, CUT, Out};
use crate::globs::{add_type_globs, searched_globs};
use crate::source::{address, locate};
use acus_search::{FileHits, FindOpts, Hit, attach_outlines, find, find_range, find_stdin};
use acus_syntax::{Address, Target};
use acus_walk::WalkOpts;
use anyhow::{Context, Result, anyhow};
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use print::{print_blocks, print_json, print_lines, print_only};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// Pattern, then paths (like rg). With -e, all positionals are paths; `-` is standard input. A path can be an address as in `acus show`: `path:A-B`, `path:N-`, `path:-N`, `path:LINE` or `path#Symbol`.
    positional: Vec<String>,
    /// Pattern (repeatable); may start with `-`.
    #[arg(short = 'e', long = "regexp", allow_hyphen_values = true)]
    patterns: Vec<String>,
    /// Include glob; prefix with ! to exclude (repeatable).
    #[arg(short = 'g', long = "glob")]
    globs: Vec<String>,
    #[arg(short = 'i', long)]
    ignore_case: bool,
    #[arg(short = 'F', long)]
    fixed_strings: bool,
    #[arg(long)]
    hidden: bool,
    /// Also search files excluded by .gitignore.
    #[arg(short = 'u', long)]
    no_ignore: bool,
    /// Follow symlinks while walking, as rg -L (grep's -L lists files without a match).
    #[arg(short = 'L', long)]
    follow: bool,
    /// Whole words only.
    #[arg(short = 'w', long = "word-regexp")]
    word: bool,
    /// Whole lines only, as grep -x: the pattern must match the entire line.
    #[arg(short = 'x', long = "line-regexp")]
    line_regexp: bool,
    /// Only list the files with hits.
    #[arg(short = 'l', long = "files-with-matches")]
    files_only: bool,
    /// Per file: the number of matching lines, then a total. Ignores --max-hits.
    #[arg(short = 'c', long)]
    count: bool,
    /// Print only the matched text, one row per match, grouped by file and line.
    #[arg(short = 'o', long = "only-matching")]
    only_matching: bool,
    /// Lines that do not match, grouped by file (capped by --max-hits).
    #[arg(short = 'v', long = "invert-match")]
    invert: bool,
    /// Stop after N matching lines per file; --max-hits still caps the total.
    #[arg(short = 'm', long = "max-count", value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    max_count: Option<u64>,
    /// Match across line breaks, as rg -U; a hit then prints all its lines. Not with -c, -v, --block, -A/-B/-C.
    #[arg(short = 'U', long = "multiline")]
    multiline: bool,
    /// Accepted for grep habits: patterns are already Rust extended regex.
    #[arg(short = 'E', long = "extended-regexp")]
    _extended: bool,
    /// Accepted for grep habits: Rust regex syntax; look-around and backreferences are rejected.
    #[arg(short = 'P', long = "perl-regexp")]
    perl: bool,
    /// grep-style --include: only files matching the glob (repeatable), same as -g GLOB.
    #[arg(long = "include", value_name = "GLOB")]
    include_globs: Vec<String>,
    /// grep-style --exclude: skip files matching the glob (repeatable), same as -g '!GLOB'.
    #[arg(long = "exclude", value_name = "GLOB")]
    exclude_globs: Vec<String>,
    /// grep-style --exclude-dir: skip directories with this name or path (repeatable), same as -g '!DIR'.
    #[arg(long = "exclude-dir", value_name = "DIR")]
    exclude_dirs: Vec<String>,
    /// File type like rg: rs, rust, py, ts, js, swift, md, … (repeatable).
    #[arg(short = 't', long = "type")]
    types: Vec<String>,
    /// Accepted for rg/grep habits; lines are always numbered, the search is always recursive.
    #[arg(short = 'n', hide = true)]
    _line_numbers: bool,
    #[arg(short = 'r', short_alias = 'R', hide = true)]
    _recursive: bool,
    #[arg(short = 'H', hide = true)]
    _with_filename: bool,
    /// Print enclosing symbol bodies instead of single lines.
    #[arg(long)]
    block: bool,
    /// Lines of context after each hit, as grep -A. Context lines print as `N-<TAB>text`
    /// (hits stay `N<TAB>text`); non-adjacent groups are split by `--`.
    #[arg(short = 'A', value_name = "N", overrides_with = "after")]
    after: Option<usize>,
    /// Lines of context before each hit, as grep -B (see -A).
    #[arg(short = 'B', value_name = "N", overrides_with = "before")]
    before: Option<usize>,
    /// Lines of context on both sides, as grep -C; an explicit -A or -B overrides it on that side.
    #[arg(short = 'C', value_name = "N", overrides_with = "context")]
    context: Option<usize>,
    #[arg(long, default_value_t = 50)]
    max_hits: usize,
    /// Per printed block.
    #[arg(long, default_value_t = 80)]
    max_lines: usize,
    /// Keep only the last N hits of each file. Applied after -m, which stops the search early;
    /// -c and -l still count every hit.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    last: Option<u64>,
    /// Clip each printed line to N characters (default 200), keeping the match in view; a clipped
    /// line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Skip tree-sitter (faster, no symbols).
    #[arg(long)]
    no_syntax: bool,
}

pub fn run(a: &Args, out: &Out) -> Result<Outcome> {
    let context = context_of(a);
    refuse_conflicts(a, context.is_some())?;
    let patterns = take_patterns(a)?;
    let newline_hint =
        !a.multiline && !a.fixed_strings && patterns.iter().any(|p| p.contains(r"\n"));
    // Named in the hint when nothing matches; the paths are those left after the pattern.
    let searched = searched_globs(&a.globs, path_args(a));
    let (roots, spans, walk_files, read_stdin) = split_paths(path_args(a))?;
    let opts = find_opts(a, patterns, roots);
    let mut files = search_all(&opts, walk_files, read_stdin, &spans)
        .map_err(|e| explain_pattern_error(e, a.fixed_strings, a.perl))?;
    if files.is_empty() {
        print_no_matches(a, out, searched.as_deref(), newline_hint);
        return Ok(Outcome::Empty);
    }
    print_files(a, out, &mut files, context);
    Ok(Outcome::Found)
}

/// `-A`, `-B` and `-C` as (before, after) when any is given. `-A` or `-B` overrides `-C` on
/// its side.
fn context_of(a: &Args) -> Option<(usize, usize)> {
    (a.before.is_some() || a.after.is_some() || a.context.is_some()).then(|| {
        (
            a.before.or(a.context).unwrap_or(0),
            a.after.or(a.context).unwrap_or(0),
        )
    })
}

/// Refuses flag combinations that cannot print what they ask for.
fn refuse_conflicts(a: &Args, context: bool) -> Result<()> {
    if a.block && context {
        return Err(capsa_core::hint::Hinted::new(
            "-A/-B/-C conflicts with --block",
            "--block prints whole symbols; -A/-B/-C print the lines around each hit",
        )
        .into());
    }
    if !a.multiline {
        return Ok(());
    }
    let refused = [
        (
            a.count,
            "-c",
            "-c counts lines, and a -U hit can span several; use -l to list files, or drop -U",
        ),
        (
            a.invert,
            "-v",
            "-v inverts line by line; drop -v, or drop -U",
        ),
        (
            a.block,
            "--block",
            "--block prints whole symbols; drop it, or use `acus show PATH#SYM`",
        ),
        (
            context,
            "-A/-B/-C",
            "context lines are per line; drop -A/-B/-C, or drop -U",
        ),
    ];
    if let Some((_, flag, hint)) = refused.iter().find(|r| r.0) {
        return Err(
            capsa_core::hint::Hinted::new(format!("{flag} conflicts with -U"), *hint).into(),
        );
    }
    Ok(())
}

/// The patterns: each `-e`, else the first positional; reads `\|` as alternation unless `-E`,
/// `-P` or `-F` is given.
fn take_patterns(a: &Args) -> Result<Vec<String>> {
    let mut patterns: Vec<String> = if a.patterns.is_empty() {
        a.positional.first().cloned().into_iter().collect()
    } else {
        a.patterns.clone()
    };
    if patterns.is_empty() {
        return Err(capsa_core::hint::Hinted::new(
            "no pattern given",
            "acus find PATTERN [PATH...] or -e PAT -e PAT",
        )
        .into());
    }
    if !a.fixed_strings && !a.perl && !a._extended {
        let mut noted = false;
        for p in &mut patterns {
            if let Some(fixed) = bre_alternation(p) {
                *p = fixed;
                noted = true;
            }
        }
        if noted {
            eprintln!(
                "note: read `\\|` as alternation `|` (grep BRE habit); write `[|]` for a literal pipe, or pass -E/-P/-F to keep `\\|` as is"
            );
        }
    }
    Ok(patterns)
}

/// The path arguments: the positionals, less the pattern when it came from a positional.
fn path_args(a: &Args) -> &[String] {
    if a.patterns.is_empty() {
        a.positional.get(1..).unwrap_or(&[])
    } else {
        &a.positional
    }
}

/// A `path:A-B` or `path#Sym` address with the line range it names.
type Spans = Vec<(Address, (usize, usize))>;

/// Splits the path arguments into walked roots and addressed line spans, and warns about
/// missing paths.
fn split_paths(paths: &[String]) -> Result<(Vec<PathBuf>, Spans, bool, bool)> {
    // `-` is standard input, searched after the walked paths.
    let read_stdin = paths.iter().any(|p| p == "-");
    // An address (`f.c:120-175`, `f.c#fn`) searches only its lines; a plain path is walked.
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut addressed: Vec<Address> = Vec::new();
    for p in paths.iter().filter(|p| *p != "-") {
        let addr = address(&capsa_core::paths::expand_home(p));
        if addr.target == Target::Whole {
            roots.push(addr.path.into());
        } else {
            addressed.push(addr);
        }
    }
    // Only `-` or addresses given: nothing to walk, and an empty root list would mean `.`.
    let walk_files = !roots.is_empty() || (!read_stdin && addressed.is_empty());
    let mut spans = Vec::new();
    let mut missing = Vec::new();
    for addr in addressed {
        if Path::new(&addr.path).exists() {
            let lines = address_lines(&addr)?;
            spans.push((addr, lines));
        } else {
            missing.push(addr.path);
        }
    }
    if let Some(first) = missing
        .first()
        .filter(|_| roots.is_empty() && spans.is_empty())
    {
        return Err(crate::source::not_found(first));
    }
    for m in &missing {
        eprintln!("warning: path not found: {m}");
    }
    Ok((roots, spans, walk_files, read_stdin))
}

/// The search options for `a`, with the patterns and the walked roots.
fn find_opts(a: &Args, patterns: Vec<String>, roots: Vec<PathBuf>) -> FindOpts {
    let (mut exclude, mut include): (Vec<String>, Vec<String>) =
        a.globs.iter().cloned().partition(|g| g.starts_with('!'));
    include.extend(a.include_globs.iter().cloned());
    exclude.extend(a.exclude_globs.iter().map(|g| format!("!{g}")));
    exclude.extend(a.exclude_dirs.iter().map(|g| format!("!{g}")));
    add_type_globs(&a.types, &mut include);
    FindOpts {
        patterns,
        walk: WalkOpts {
            roots,
            include,
            // A trailing `/` ("!node_modules/") names a directory; globs match the bare name.
            exclude: exclude
                .into_iter()
                .map(|g| g[1..].trim_end_matches('/').to_owned())
                .collect(),
            hidden: a.hidden,
            no_ignore: a.no_ignore,
            follow: a.follow,
        },
        ignore_case: a.ignore_case,
        fixed: a.fixed_strings,
        // Outlines are parsed below, for the shown files only.
        syntax: false,
        word: a.word,
        line: a.line_regexp,
        invert: a.invert,
        max_per_file: a.max_count.map(|n| n as usize),
        multiline: a.multiline,
    }
}

/// The stderr notes for a search with no hits; JSON output stays silent.
fn print_no_matches(a: &Args, out: &Out, searched: Option<&str>, newline_hint: bool) {
    if out.format != Format::Json && !(a.hidden && a.no_ignore) {
        eprintln!("(no matches; hidden and .gitignored files were skipped: --hidden, -u)");
    }
    if out.format != Format::Json
        && let Some(s) = searched
    {
        eprintln!("(searched {s})");
    }
    if newline_hint && out.format != Format::Json {
        eprintln!("hint: \\n matches only across lines; add -U to match across line breaks");
    }
}

/// Prints the files with hits as counts, file names or hit lines, within the `--head` budget.
fn print_files(a: &Args, out: &Out, files: &mut [FileHits], context: Option<(usize, usize)>) {
    if a.count && out.format == Format::Json {
        let total: usize = files.iter().map(|f| f.hits.len()).sum();
        let counts: Vec<_> = files
            .iter()
            .map(|f| serde_json::json!({ "path": f.path, "count": f.hits.len() }))
            .collect();
        println!("{}", serde_json::json!({ "files": counts, "total": total }));
        return;
    }
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    if a.count {
        let total: usize = files.iter().map(|f| f.hits.len()).sum();
        for f in &*files {
            budget.emit(&format!("{} {}", f.path, f.hits.len()));
        }
        budget.emit(&format!("total {total} in {} files", files.len()));
        if let Some(f) = budget.footer() {
            println!("{f}");
        }
        return;
    }
    if a.files_only && out.format != Format::Json {
        for f in &*files {
            budget.emit(&format!("{} {}", f.path, f.hits.len()));
        }
        if let Some(f) = budget.footer() {
            println!("{f}");
        }
        return;
    }
    if let Some(n) = a.last {
        for f in &mut *files {
            let cut = f.hits.len().saturating_sub(n as usize);
            f.hits.drain(..cut);
        }
    }
    if !a.no_syntax {
        // Files with no budget left are not printed, so they stay unparsed. Shown files are a
        // prefix because the budget is spent in path order.
        let k = take_budget(files, a.max_hits).0.len();
        attach_outlines(&mut files[..k]);
    }
    let (shown, rest, rest_files) = take_budget(files, a.max_hits);
    let cut = a.cut.map_or(CUT, |n| n as usize);
    match out.format {
        Format::Json => print_json(&shown, rest, a.only_matching),
        _ if a.only_matching => print_only(&shown, out, cut, &mut budget),
        _ if a.block => print_blocks(&shown, out, a.max_lines, cut, &mut budget),
        _ => print_lines(&shown, out, context, cut, &mut budget),
    }
    if rest > 0 && out.format != Format::Json {
        budget.emit(&format!(
            "… {rest} more hits in {rest_files} files (narrow with -g/-e or raise --max-hits)"
        ));
    }
    if let Some(f) = budget.footer() {
        println!("{f}");
    }
}

/// Spends the hit budget in path order: the shown files with their hit counts, then the
/// number of hits and files that were cut off.
fn take_budget(files: &[FileHits], max_hits: usize) -> (Vec<(&FileHits, usize)>, usize, usize) {
    let mut budget = max_hits;
    let mut shown = Vec::new();
    let (mut rest, mut rest_files) = (0, 0);
    for f in files {
        let n = f.hits.len().min(budget);
        budget -= n;
        if n > 0 {
            shown.push((f, n));
        }
        if n < f.hits.len() {
            rest += f.hits.len() - n;
            rest_files += 1;
        }
    }
    (shown, rest, rest_files)
}

/// Turns a regex compile error into a hint; other errors pass through unchanged.
fn explain_pattern_error(e: anyhow::Error, fixed: bool, perl: bool) -> anyhow::Error {
    let msg = format!("{e:#}");
    if fixed || !msg.starts_with("invalid pattern") {
        e
    } else if msg.contains("look-around") || msg.contains("backreference") {
        let p = if perl { "-P is accepted, but " } else { "" };
        capsa_core::hint::Hinted::new(
            msg,
            format!(
                "{p}acus uses Rust regex, which has no look-around or backreferences; match the surrounding text directly, or narrow with -w / a second -e"
            ),
        )
        .into()
    } else {
        capsa_core::hint::Hinted::new(
            msg,
            "escape regex characters like ( [ { . with \\, or use -F for literal text",
        )
        .into()
    }
}
/// The 1-based inclusive lines an address names in its file, resolved as `acus show` resolves
/// them.
fn address_lines(addr: &Address) -> Result<(usize, usize)> {
    let src = std::fs::read_to_string(&addr.path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => anyhow!("{}: binary file, not shown", addr.path),
        _ => anyhow!("{}: {e}", addr.path),
    })?;
    let lines: Vec<&str> = src.lines().collect();
    let span = locate(addr, &addr.path, &src, &lines, &mut HashMap::new())?;
    Ok((span.start, span.end))
}

/// Files with hits: the walked paths, the addressed line ranges, then standard input when `-`
/// was given. Sorted by path.
fn search_all(
    opts: &FindOpts,
    walk_files: bool,
    read_stdin: bool,
    spans: &[(Address, (usize, usize))],
) -> Result<Vec<FileHits>> {
    let mut files = if walk_files { find(opts)? } else { Vec::new() };
    for (addr, lines) in spans {
        files.extend(find_range(opts, Path::new(&addr.path), *lines)?);
    }
    if read_stdin {
        let mut input = Vec::new();
        std::io::stdin()
            .read_to_end(&mut input)
            .context("cannot read stdin")?;
        files.extend(find_stdin(opts, &input)?);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// grep's BRE spells alternation `\|`; Rust regex reads it as a literal pipe. Agents write the
/// grep form from habit, so unless the pattern already uses an unescaped `|` (a sign the author
/// knows ERE) each `\|` outside a character class becomes `|`. `\\|` is an escaped backslash
/// followed by such a `|`, so it leaves the pattern as is. Returns None when nothing changes.
fn bre_alternation(p: &str) -> Option<String> {
    let mut out = String::with_capacity(p.len());
    let (mut in_class, mut changed) = (false, false);
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('|') if !in_class => {
                    out.push('|');
                    changed = true;
                }
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            '|' if !in_class => return None,
            _ => out.push(c),
        }
    }
    changed.then_some(out)
}

pub(super) fn matched(h: &Hit) -> impl Iterator<Item = &str> {
    h.spans.iter().filter_map(|&(a, b)| h.text.get(a..b))
}
