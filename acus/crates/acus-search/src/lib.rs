// SPDX-License-Identifier: MPL-2.0
//! Regex search over files with enclosing-symbol context.

use acus_syntax::{Lang, Outline, outline};
use acus_walk::{WalkOpts, display_path, walk};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, sinks::Lossy};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    sync::Mutex,
};

/// Reports why a search could not run.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    /// The searched paths cannot be walked.
    #[error(transparent)]
    Walk(#[from] acus_walk::WalkError),
    /// The pattern does not compile.
    #[error("invalid pattern")]
    Pattern(#[source] grep_regex::Error),
    /// A file cannot be read.
    #[error("cannot read {path}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

thread_local! {
    /// One read buffer per walker thread, so a file costs no allocation beyond its `source`.
    static BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// The searcher for the last `(multiline, invert)` pair this thread used; every file of one
    /// `find` run has the same pair, so it is built once per thread.
    static SEARCHER: RefCell<Option<((bool, bool), Searcher)>> = const { RefCell::new(None) };
}

/// A buffer above this size is released after use instead of kept for the next file.
const KEEP_BUF: usize = 8 << 20;

pub struct FindOpts {
    pub patterns: Vec<String>,
    pub walk: WalkOpts,
    pub ignore_case: bool,
    /// Treat patterns as literal strings.
    pub fixed: bool,
    /// Parse files with hits to attach enclosing symbols.
    pub syntax: bool,
    /// Match whole words only.
    pub word: bool,
    /// Match whole lines only (grep -x): the pattern is wrapped as `^(?:PAT)$`.
    pub line: bool,
    /// Report the lines that do not match (grep -v).
    pub invert: bool,
    /// Stop after this many hit lines per file (grep -m).
    pub max_per_file: Option<usize>,
    /// Let patterns match across line breaks (rg -U); a hit then covers every line of its match.
    pub multiline: bool,
}

#[derive(Debug, Serialize)]
pub struct Hit {
    pub line: usize,
    /// The line; with multiline search, every line the match covers, joined by `\n`.
    /// Byte offset of the first match in `text`.
    #[serde(skip)]
    pub col: usize,
    pub text: String,
    /// Byte ranges of every match in `text`; empty for inverted hits.
    #[serde(skip)]
    pub spans: Vec<(usize, usize)>,
}

#[derive(Debug, Serialize)]
pub struct FileHits {
    pub path: String,
    pub lang: Option<Lang>,
    pub hits: Vec<Hit>,
    /// File contents, kept so callers can print blocks without re-reading.
    #[serde(skip)]
    pub source: String,
    #[serde(skip)]
    pub outline: Option<Outline>,
    /// The lines a `find_ranges` search covered, 1-based inclusive; None for a whole file.
    #[serde(skip)]
    pub lines: Option<(usize, usize)>,
}

/// One line of a grep-style view from [`FileHits::context_rows`].
#[derive(Debug)]
pub enum Row<'a> {
    /// A hit: a matching line (a non-matching one with -v).
    Hit(&'a Hit),
    /// A line around a hit (grep -A/-B/-C).
    Context { line: usize, text: &'a str },
    /// grep's `--`: the rows on either side are not adjacent.
    Break,
}

impl FileHits {
    /// The first `n` hits with `before` lines above and `after` lines below each, in line order.
    /// Overlapping and adjacent ranges merge; a `Break` separates non-adjacent groups.
    pub fn context_rows(&self, n: usize, before: usize, after: usize) -> Vec<Row<'_>> {
        let total = self.source.lines().count();
        // Context stays inside the searched range, so `path:A-B` never prints lines outside it.
        let (first, last) = self.lines.unwrap_or((1, total));
        let (first, last) = (first.max(1), last.min(total));
        let hits: BTreeMap<usize, &Hit> = self.hits[..n].iter().map(|h| (h.line, h)).collect();
        let mut shown = BTreeSet::new();
        for &line in hits.keys() {
            let end = line.saturating_add(after).min(last);
            shown.extend(line.saturating_sub(before).max(first)..=end);
        }
        // The lines are read in order, once, as the shown line numbers ascend.
        let mut numbered = self
            .source
            .lines()
            .enumerate()
            .map(|(i, text)| (i + 1, text))
            .peekable();
        let mut rows = Vec::with_capacity(shown.len());
        let mut prev = None;
        for line in shown {
            if prev.is_some_and(|p| line > p + 1) {
                rows.push(Row::Break);
            }
            prev = Some(line);
            while numbered.next_if(|&(k, _)| k < line).is_some() {}
            let text = numbered
                .next_if(|&(k, _)| k == line)
                .map_or("", |(_, text)| text);
            rows.push(match hits.get(&line) {
                Some(&h) => Row::Hit(h),
                None => Row::Context { line, text },
            });
        }
        rows
    }
}

/// Files with at least one hit, sorted by path.
pub fn find(o: &FindOpts) -> Result<Vec<FileHits>, FindError> {
    let matcher = matcher(o)?;
    let out = Mutex::new(Vec::new());
    walk(&o.walk, |path| {
        // Search before locking, or the threads take turns.
        // ponytail: unreadable files are skipped like rg does without --debug
        let found = BUF.with(|b| {
            let mut buf = b.borrow_mut();
            buf.clear();
            let read = std::fs::File::open(path).and_then(|mut f| {
                // A buffer taken as a file's `source` comes back empty; sizing it once avoids the
                // repeated growth of `read_to_end`.
                if buf.capacity() == 0 {
                    let len = f.metadata().map_or(0, |m| m.len());
                    buf.reserve(usize::try_from(len).unwrap_or(0));
                }
                f.read_to_end(&mut buf)
            });
            let found = read
                .ok()
                .and_then(|_| search(o, &matcher, path, &mut buf, None));
            if buf.capacity() > KEEP_BUF {
                *buf = Vec::new();
            }
            found
        });
        if let Some(f) = found {
            out.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(f);
        }
    })?;
    let mut v = out
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(v)
}

/// Searches standard input like a file named `<stdin>`; `None` when nothing matches.
pub fn find_stdin(o: &FindOpts, input: &[u8]) -> Result<Option<FileHits>, FindError> {
    let matcher = matcher(o)?;
    Ok(search(
        o,
        &matcher,
        Path::new("<stdin>"),
        &mut input.to_vec(),
        None,
    ))
}

/// Searches each `(path, lines)` range, keeping only hits on lines `first..=last` (1-based,
/// inclusive). Line numbers stay those of the whole file. The pattern compiles once for all of
/// them; a range with no hit gives no entry.
pub fn find_ranges<'a>(
    o: &FindOpts,
    ranges: impl IntoIterator<Item = (&'a Path, (usize, usize))>,
) -> Result<Vec<FileHits>, FindError> {
    let mut ranges = ranges.into_iter().peekable();
    if ranges.peek().is_none() {
        return Ok(Vec::new());
    }
    let matcher = matcher(o)?;
    let mut found = Vec::new();
    for (path, lines) in ranges {
        let mut bytes = std::fs::read(path).map_err(|source| FindError::Read {
            path: path.display().to_string(),
            source,
        })?;
        found.extend(search(o, &matcher, path, &mut bytes, Some(lines)));
    }
    Ok(found)
}

fn matcher(o: &FindOpts) -> Result<RegexMatcher, FindError> {
    let pats: Vec<String> = o
        .patterns
        .iter()
        .map(|p| {
            let p = if o.fixed { regex::escape(p) } else { p.clone() };
            if o.line { format!("^(?:{p})$") } else { p }
        })
        .collect();
    let mut b = RegexMatcherBuilder::new();
    // With multi_line, `^` and `$` match at line breaks, so -x holds for each line of a -U match
    // and, as in ripgrep, `^fn foo\nbar` anchors at the start of any line.
    b.case_insensitive(o.ignore_case)
        .word(o.word)
        .multi_line(o.line || o.multiline);
    if o.multiline {
        // The default line terminator bans `\n` in patterns.
        b.line_terminator(None);
    }
    b.build_many(&pats).map_err(FindError::Pattern)
}

/// Fills `outline` for each file that has a language and none yet, in parallel. `find` with
/// `syntax: false` leaves them empty, so a caller parses only the files it prints.
pub fn attach_outlines(files: &mut [FileHits]) {
    let todo = files
        .iter()
        .filter(|f| f.outline.is_none() && f.lang.is_some())
        .count();
    if todo == 0 {
        return;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = files.len().div_ceil(threads.min(todo).max(1));
    std::thread::scope(|s| {
        for part in files.chunks_mut(chunk) {
            s.spawn(move || {
                for f in part {
                    if f.outline.is_none()
                        && let Some(l) = f.lang
                    {
                        f.outline = outline(l, &f.source);
                    }
                }
            });
        }
    });
}

/// Searches `bytes`, which the file hits take as their `source` when there are any: the buffer
/// is moved, not copied, so `bytes` is empty afterwards in that case.
fn search(
    o: &FindOpts,
    matcher: &RegexMatcher,
    path: &Path,
    bytes: &mut Vec<u8>,
    lines: Option<(usize, usize)>,
) -> Option<FileHits> {
    let key = (o.multiline, o.invert);
    let mut searcher = SEARCHER
        .with(|c| c.borrow_mut().take())
        .filter(|(k, _)| *k == key)
        .map_or_else(
            || {
                SearcherBuilder::new()
                    .binary_detection(BinaryDetection::quit(0))
                    .line_number(true)
                    .multi_line(o.multiline)
                    .invert_match(o.invert)
                    .build()
            },
            |(_, s)| s,
        );
    let hits = search_with(&mut searcher, o, matcher, bytes, lines);
    SEARCHER.with(|c| *c.borrow_mut() = Some((key, searcher)));
    hits.map(|hits| file_hits(o, path, bytes, hits, lines))
}

fn file_hits(
    o: &FindOpts,
    path: &Path,
    bytes: &mut Vec<u8>,
    hits: Vec<Hit>,
    lines: Option<(usize, usize)>,
) -> FileHits {
    let lang = Lang::from_path(path);
    let source = source_of(bytes);
    let outline = lang.filter(|_| o.syntax).and_then(|l| outline(l, &source));
    FileHits {
        path: display_path(path),
        lang,
        hits,
        source,
        outline,
        lines,
    }
}

/// Takes the bytes as text. Invalid UTF-8 becomes replacement characters, as `find` has always
/// printed it.
fn source_of(bytes: &mut Vec<u8>) -> String {
    match String::from_utf8(std::mem::take(bytes)) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

fn search_with(
    searcher: &mut Searcher,
    o: &FindOpts,
    matcher: &RegexMatcher,
    bytes: &[u8],
    lines: Option<(usize, usize)>,
) -> Option<Vec<Hit>> {
    let mut hits = Vec::new();
    let sink = Lossy(|n, line| {
        if lines.is_some_and(|(first, last)| !(first..=last).contains(&(n as usize))) {
            return Ok(true);
        }
        let text = line.trim_end_matches(['\n', '\r']);
        let mut spans = Vec::new();
        if !o.invert {
            let _ = matcher.find_iter(text.as_bytes(), |m| {
                // An empty match would print nothing and could loop on its own position.
                if m.end() > m.start() {
                    spans.push((m.start(), m.end()));
                }
                true
            });
        }
        let col = spans.first().map_or(0, |s| s.0);
        hits.push(Hit {
            line: n as usize,
            col,
            text: text.to_owned(),
            spans,
        });
        Ok(o.max_per_file.is_none_or(|m| hits.len() < m))
    });
    if searcher.search_slice(matcher, bytes, sink).is_err() || hits.is_empty() {
        return None;
    }
    Some(hits)
}
