// SPDX-License-Identifier: MPL-2.0
//! `acus map`: the repository tree with line counts and top-level symbols, within a budget.

use crate::hinted;
mod render;

use crate::out::{Budget, clip};
use crate::source::warn_missing_roots;
use acus_syntax::{Lang, outline};
use acus_walk::{WalkOpts, display_path, walk};
use anyhow::Result;
use capsa_core::exit::Outcome;
use render::{dir_row, file_rows, has_closed, render, summary, tags};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(clap::Args)]
pub struct Args {
    /// Directories to map (default: the current directory).
    #[arg(value_name = "PATH", value_parser = crate::home::<PathBuf>)]
    paths: Vec<PathBuf>,
    /// Output budget in tokens (about 4 characters each); the largest directories are opened
    /// first until it is spent.
    #[arg(long, default_value_t = 400)]
    budget: usize,
    /// Open directories at most N levels below PATH.
    #[arg(long)]
    depth: Option<usize>,
    #[arg(long)]
    hidden: bool,
    /// Stop after N printed lines, headers and footers included; the rest is counted in a footer.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    head: Option<u64>,
    /// Clip each printed line to N characters; a clipped line ends with `… (+N chars)`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    cut: Option<u64>,
}

/// Generated or vendored text that would only inflate the counts.
const SKIP_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "go.sum",
    "poetry.lock",
    "uv.lock",
    "Package.resolved",
    "Gemfile.lock",
    "composer.lock",
];
/// Bundle directories whose contents are tool-managed.
const SKIP_DIRS: &[&str] = &["xcodeproj", "xcworkspace", "xcassets", "xcframework"];
/// Files larger than this are data, not code.
const MAX_BYTES: u64 = 1 << 20;
/// Manifests that mark a project root, and the tag shown for it.
const MANIFESTS: &[(&str, &str)] = &[
    ("Cargo.toml", "rust"),
    ("Package.swift", "swift"),
    ("package.json", "node"),
    ("go.mod", "go"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("pom.xml", "java"),
    ("build.gradle", "gradle"),
    ("build.gradle.kts", "gradle"),
    ("CMakeLists.txt", "cmake"),
    ("Makefile", "make"),
];
/// File stems listed first: where a reader starts.
const ENTRY: &[&str] = &[
    "main", "lib", "mod", "index", "app", "cli", "__init__", "__main__",
];
/// A directory's files get one line each with their symbols only up to this many.
const DETAIL_FILES: usize = 12;
const FILE_NAMES: usize = 6;
const SYMBOLS: usize = 6;
/// Directories opened after sources of the same size.
const SECONDARY: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "docs",
    "doc",
    "fixtures",
    "snapshots",
    "examples",
    "benches",
    "testdata",
];

#[derive(Default)]
struct Dir {
    files: Vec<File>,
    dirs: BTreeMap<String, Dir>,
    n: usize,
    lines: usize,
    exts: BTreeMap<String, usize>,
    tags: BTreeSet<&'static str>,
    open: bool,
    /// Files on their own lines with top-level symbols.
    detail: bool,
}

struct File {
    name: String,
    path: PathBuf,
    lines: usize,
    symbols: Vec<String>,
}

pub fn run(a: Args) -> Result<Outcome> {
    let paths = if a.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        a.paths
    };
    if let Some(p) = paths.iter().find(|p| p.is_file()) {
        let shown = display_path(p);
        return Err(hinted(
            format!("{shown} is a file"),
            format!("acus outline {shown} lists its symbols"),
        ));
    }
    let found = Mutex::new(Vec::new());
    let opts = WalkOpts {
        roots: paths.clone(),
        include: vec![],
        exclude: SKIP_DIRS.iter().map(|e| format!("*.{e}")).collect(),
        hidden: a.hidden,
        no_ignore: false,
        ..Default::default()
    };
    warn_missing_roots(&opts);
    walk(&opts, |f| {
        if let Some(n) = count_lines(f) {
            found
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((f.to_path_buf(), n));
        }
    })?;
    let mut found = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    found.sort();
    found.dedup_by(|a, b| a.0 == b.0);
    let base = base_of(&paths);
    let mut root = Dir::default();
    for (f, n) in found {
        let rel = f.strip_prefix(&base).unwrap_or(&f);
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        insert(&mut root, &parts, f, n);
    }
    if root.n == 0 {
        return Ok(Outcome::Empty);
    }
    roll_up(&mut root);
    load_symbols(&mut root);
    plan(&mut root, a.budget * 4, a.depth.unwrap_or(usize::MAX));
    let mut budget = Budget::new(a.head.map(|n| n as usize));
    let cut = a.cut.map(|n| n as usize);
    let clip_row = |s: &str| cut.map_or_else(|| s.to_owned(), |n| clip(s, 0, n));
    budget.emit(&clip_row(&format!(
        "== {}{}  {}",
        display_path(&base),
        tags(&root),
        summary(&root)
    )));
    let mut rows = Vec::new();
    render(&root, 1, &mut rows);
    for r in &rows {
        budget.emit(&clip_row(r));
    }
    if has_closed(&root) {
        budget.emit(&clip_row(
            "… deeper: acus map DIR; symbols: acus outline DIR --depth 1",
        ));
    }
    if let Some(f) = budget.footer() {
        outln!("{f}");
    }
    Ok(Outcome::Found)
}

/// The deepest path shared by every root, so rows are relative to what they have in common.
fn base_of(paths: &[PathBuf]) -> PathBuf {
    let mut base = paths.first().map_or_else(PathBuf::new, Clone::clone);
    for p in &paths[1..] {
        base = base
            .components()
            .zip(p.components())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.as_os_str())
            .collect();
    }
    if base.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        base
    }
}

/// Line count of a text file; binaries, lockfiles and large data files give `None`.
fn count_lines(f: &Path) -> Option<usize> {
    let name = f.file_name()?.to_str()?;
    if SKIP_FILES.contains(&name)
        || ["LICENSE", "LICENCE", "COPYING", "NOTICE"]
            .iter()
            .any(|p| name.starts_with(p))
        || name.ends_with(".min.js")
        || name.ends_with(".map")
        || f.metadata().ok()?.len() > MAX_BYTES
    {
        return None;
    }
    let b = std::fs::read(f).ok()?;
    if b.is_empty() || b[..b.len().min(8000)].contains(&0) {
        return None;
    }
    Some(b.iter().filter(|&&c| c == b'\n').count() + usize::from(!b.ends_with(b"\n")))
}

fn insert(d: &mut Dir, parts: &[String], path: PathBuf, lines: usize) {
    d.n += 1;
    d.lines += lines;
    let name = &parts[parts.len() - 1];
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    if !ext.is_empty() {
        *d.exts.entry(ext.to_string()).or_default() += lines;
    }
    match parts {
        [name] => {
            if let Some((_, tag)) = MANIFESTS.iter().find(|(m, _)| m == name) {
                d.tags.insert(tag);
            }
            d.files.push(File {
                name: name.clone(),
                path,
                lines,
                symbols: vec![],
            });
        }
        [dir, rest @ ..] => insert(d.dirs.entry(dir.clone()).or_default(), rest, path, lines),
        [] => {}
    }
}

/// Merges directories that only hold one directory into it (`a/b/c/` is one line) and
/// directories holding a single file into their parent's files (`benches/core.rs`).
#[expect(
    clippy::unwrap_used,
    reason = "each branch checked the length it pops from"
)]
fn roll_up(d: &mut Dir) {
    let mut dirs = BTreeMap::new();
    for (mut name, mut c) in std::mem::take(&mut d.dirs) {
        while c.files.is_empty() && c.dirs.len() == 1 {
            let (sub, inner) = c.dirs.pop_first().unwrap();
            name = format!("{name}/{sub}");
            c = inner;
        }
        roll_up(&mut c);
        if c.dirs.is_empty() && c.files.len() == 1 {
            let mut f = c.files.pop().unwrap();
            f.name = format!("{name}/{}", f.name);
            d.files.push(f);
        } else {
            dirs.insert(name, c);
        }
    }
    d.dirs = dirs;
}

/// Characters of rows, newlines included.
fn chars(rows: &[String]) -> usize {
    rows.iter().map(|r| r.chars().count() + 1).sum()
}

/// Characters a directory adds when opened: its file line and subdirectory lines.
fn open_cost(d: &Dir, level: usize) -> usize {
    let mut rows = file_rows(d, level, false);
    let pad = "  ".repeat(level - 1);
    rows.extend(d.dirs.iter().map(|(n, c)| dir_row(&pad, n, c)));
    chars(&rows)
}

/// Characters the files of an open directory add on their own lines with symbols.
fn detail_cost(d: &Dir, level: usize) -> usize {
    chars(&file_rows(d, level, true)).saturating_sub(chars(&file_rows(d, level, false)))
}

enum Step {
    Open,
    Detail,
}

/// Greedily opens the directory or file list holding the most lines while the budget allows,
/// so large repositories stay at directory level and small ones reach their symbols.
#[expect(clippy::unwrap_used, reason = "every path was inserted by the walk")]
fn plan(root: &mut Dir, budget: usize, max_depth: usize) {
    root.open = true;
    let mut used = open_cost(root, 1);
    loop {
        let mut best: Option<(usize, Vec<String>, Step, usize)> = None;
        candidates(
            root,
            &mut vec![],
            1,
            max_depth,
            &mut |lines, path, step, cost| {
                if used + cost <= budget && best.as_ref().is_none_or(|b| lines > b.0) {
                    best = Some((lines, path.to_vec(), step, cost));
                }
            },
        );
        let Some((_, path, step, cost)) = best else {
            break;
        };
        let d = path
            .iter()
            .fold(&mut *root, |d, k| d.dirs.get_mut(k).unwrap());
        match step {
            Step::Open => d.open = true,
            Step::Detail => d.detail = true,
        }
        used += cost;
    }
}

fn candidates(
    d: &Dir,
    path: &mut Vec<String>,
    depth: usize,
    max_depth: usize,
    f: &mut impl FnMut(usize, &[String], Step, usize),
) {
    if !d.detail && d.files.iter().any(|f| !f.symbols.is_empty()) {
        let lines = d.files.iter().map(|f| f.lines).sum();
        f(
            weight(path, lines),
            path,
            Step::Detail,
            detail_cost(d, depth),
        );
    }
    for (name, c) in &d.dirs {
        path.push(name.clone());
        if c.open {
            candidates(c, path, depth + 1, max_depth, f);
        } else if depth < max_depth {
            f(
                weight(path, c.lines),
                path,
                Step::Open,
                open_cost(c, depth + 1),
            );
        }
        path.pop();
    }
}

/// Opening priority: tests, docs and fixtures count a quarter, so sources open first.
fn weight(path: &[String], lines: usize) -> usize {
    let secondary = path
        .iter()
        .flat_map(|p| p.split('/'))
        .any(|c| SECONDARY.contains(&c.to_ascii_lowercase().as_str()));
    if secondary { lines / 4 } else { lines }
}

/// Symbols of the files in small directories, the only ones that can show them.
fn load_symbols(root: &mut Dir) {
    // Reading and parsing each file is independent work, so the files are split over
    // threads. Each file keeps its slot, so the output order does not change.
    let mut files = Vec::new();
    collect_detail_files(root, &mut files);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = files.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for part in files.chunks_mut(chunk) {
            s.spawn(move || {
                for f in part {
                    f.symbols = top_symbols(&f.path);
                }
            });
        }
    });
}

/// The files of the directories small enough to show their symbols, in tree order.
fn collect_detail_files<'a>(d: &'a mut Dir, out: &mut Vec<&'a mut File>) {
    let Dir { files, dirs, .. } = d;
    if files.len() <= DETAIL_FILES {
        out.extend(files.iter_mut());
    }
    for sub in dirs.values_mut() {
        collect_detail_files(sub, out);
    }
}

/// Names of a code file's top-level symbols, deduplicated (`impl Foo` and `struct Foo`).
fn top_symbols(path: &Path) -> Vec<String> {
    let Some(lang) = Lang::from_path(path)
        .filter(|l| !matches!(l, Lang::Toml | Lang::Json | Lang::Yaml | Lang::Markdown))
    else {
        return vec![];
    };
    let Some(o) = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| outline(lang, &s))
    else {
        return vec![];
    };
    let mut seen = BTreeSet::new();
    o.symbols
        .into_iter()
        .filter(|s| s.depth == 0 && seen.insert(s.name.clone()))
        .map(|s| s.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, usize)]) -> Dir {
        let mut root = Dir::default();
        for (p, n) in files {
            let parts: Vec<String> = p.split('/').map(String::from).collect();
            insert(&mut root, &parts, PathBuf::from(p), *n);
        }
        roll_up(&mut root);
        root
    }

    fn rows(root: &mut Dir, budget: usize) -> Vec<String> {
        plan(root, budget, usize::MAX);
        let mut out = Vec::new();
        render(root, 1, &mut out);
        out
    }

    #[test]
    fn rolls_up_chains_and_single_files_and_tags_projects() {
        let mut root = tree(&[
            ("Cargo.toml", 20),
            ("benches/core.rs", 107),
            ("crates/core/src/lib.rs", 300),
            ("crates/core/src/util.rs", 1200),
        ]);
        assert_eq!(
            rows(&mut root, 0),
            [
                "benches/core.rs 107, Cargo.toml 20",
                "crates/core/src/  2 files, 1.5k lines (rs)"
            ]
        );
        assert_eq!(tags(&root), " [rust]");
    }

    #[test]
    fn opens_the_largest_directories_within_the_budget() {
        let mut root = tree(&[
            ("big/a/x.rs", 900),
            ("big/a/w.rs", 100),
            ("big/b/y.rs", 800),
            ("big/b/v.rs", 1),
            ("small/c/z.rs", 10),
            ("small/c/u.rs", 5),
            ("small/d/q.rs", 3),
            ("small/d/r.rs", 3),
        ]);
        let budget = open_cost(&root, 1) + open_cost(&root.dirs["big"], 2);
        assert_eq!(
            rows(&mut root, budget),
            [
                "big/  4 files, 1.8k lines (rs)",
                "  a/  2 files, 1.0k lines (rs)",
                "  b/  2 files, 801 lines (rs)",
                "small/  4 files, 21 lines (rs)"
            ]
        );
    }

    #[test]
    fn lists_entry_points_first_and_caps_file_names() {
        let names: Vec<_> = (0..8).map(|i| format!("f{i}.rs")).collect();
        let mut files: Vec<(&str, usize)> = names.iter().map(|n| (n.as_str(), 50)).collect();
        files.push(("main.rs", 5));
        let mut root = tree(&files);
        root.open = true;
        let mut out = Vec::new();
        render(&root, 1, &mut out);
        assert_eq!(
            out,
            ["main.rs 5, f0.rs 50, f1.rs 50, f2.rs 50, f3.rs 50, f4.rs 50 +3"]
        );
    }

    #[test]
    fn small_trees_reach_their_symbols() {
        let dir = std::env::temp_dir().join(format!("acus-map-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "struct A;\nimpl A { fn f() {} }\nfn g() {}\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn h() {}\n").unwrap();
        let mut root = tree(&[]);
        for f in ["src/lib.rs", "src/b.rs"] {
            let parts: Vec<String> = f.split('/').map(String::from).collect();
            insert(
                &mut root,
                &parts,
                dir.join(f),
                count_lines(&dir.join(f)).unwrap(),
            );
        }
        load_symbols(&mut root);
        plan(&mut root, 1000, usize::MAX);
        let mut out = Vec::new();
        render(&root, 1, &mut out);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            out,
            [
                "src/  2 files, 4 lines (rs)",
                "  lib.rs 3: A, g",
                "  b.rs 1: h"
            ]
        );
    }
}
