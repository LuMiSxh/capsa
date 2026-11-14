// SPDX-License-Identifier: MPL-2.0
//! Call-graph walk behind `acus follow`: callers and callees of a symbol, found by searching
//! the repo for its name and resolving each call site.

use acus_search::{FileHits, FindError, FindOpts, find};
use acus_syntax::calls::{self, Call, CallForm};
use acus_syntax::{Lang, Outline, Resolve, Symbol, resolve};
use acus_walk::WalkOpts;
use std::collections::HashMap;
use std::path::Path;

mod callees;
mod callers;

pub use callees::callees;
pub use callers::callers;

/// Kinds a call can target: functions, methods and the types a constructor call names.
pub const CALLABLE: &[&str] = &[
    "fn", "method", "def", "func", "function", "macro", "class", "struct", "enum",
];

/// A symbol that `follow` walks from: its file, outline entry and call-site language.
#[derive(Clone)]
pub struct Node {
    pub path: String,
    pub sym: Symbol,
    /// Name matched against call sites: the symbol's name, or its type for constructors.
    pub name: String,
    /// The enclosing type's name, if the symbol sits inside one.
    pub container: Option<String>,
    /// The address this node prints as: `path#Qual`, or `path:A-B` when the qualified name
    /// does not resolve back to it alone.
    pub addr: String,
    pub lang: Lang,
}

/// `path#Qual` when the qualified name resolves back to this symbol alone, else `path:A-B`;
/// either one is an address `acus show` accepts.
pub fn addr_of(path: &str, o: &Outline, s: &Symbol) -> String {
    match resolve(o, &s.qual) {
        Resolve::Found(f) if f == s => format!("{path}#{}", s.qual),
        _ => format!("{path}:{}-{}", s.start_line, s.end_line),
    }
}

/// The enclosing type of a qualified name: `Parser` for `Parser::parse`.
pub fn container_of(qual: &str) -> Option<String> {
    qual.rsplit_once("::")
        .map(|(p, _)| p.rsplit("::").next().unwrap_or(p).to_owned())
}

/// Languages whose call sites are searched together: TS, TSX and JS share one family.
fn family(l: Lang) -> u8 {
    match l {
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => 0,
        Lang::Rust => 1,
        Lang::Swift => 2,
        Lang::Python => 3,
        Lang::C => 4,
        Lang::Shell => 5,
        _ => 6,
    }
}

/// Glob for every extension of the target's language family.
fn family_globs(lang: Lang) -> Vec<String> {
    Lang::extensions()
        .filter(|e| {
            Lang::from_path(Path::new(&format!("f.{e}"))).is_some_and(|l| family(l) == family(lang))
        })
        .map(|e| format!("*.{e}"))
        .collect()
}

/// Files of the target's family with any of `names` as a whole word: one pass for all names.
fn search(names: &[String], lang: Lang) -> Result<Vec<FileHits>, FindError> {
    find(&FindOpts {
        patterns: names.to_vec(),
        walk: WalkOpts {
            include: family_globs(lang),
            ..Default::default()
        },
        ignore_case: false,
        fixed: true,
        syntax: false,
        word: true,
        line: false,
        invert: false,
        max_per_file: None,
        multiline: false,
    })
}

/// Outline, call sites and source of one file.
type Analyzed = (Outline, Vec<Call>, String);

/// Per-run cache of analysed files, keyed by path.
type Memo = HashMap<String, Option<Analyzed>>;

/// Analyses `items` on scoped threads, in chunks. The results come back in input order; the
/// caller inserts them serially, so the memo never depends on thread timing.
fn par_analyse<T: Sync>(
    items: &[T],
    work: impl Fn(&T) -> (String, Option<Analyzed>) + Sync,
) -> Vec<(String, Option<Analyzed>)> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    if items.len() < 2 || threads < 2 {
        return items.iter().map(work).collect();
    }
    let chunk = items.len().div_ceil(threads);
    let work = &work;
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(work).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

/// Fills `memo` for every hit file not cached yet, parsing them in parallel.
fn prefetch(memo: &mut Memo, files: &[FileHits]) {
    let mut seen = std::collections::HashSet::new();
    let todo: Vec<&FileHits> = files
        .iter()
        .filter(|f| !memo.contains_key(&f.path) && seen.insert(f.path.as_str()))
        .collect();
    let done = par_analyse(&todo, |f| {
        let a = f
            .lang
            .and_then(|l| calls::analyze(l, &f.source))
            .map(|(o, c)| (o, c, f.source.clone()));
        (f.path.clone(), a)
    });
    memo.extend(done);
}

/// Analyses one file read from disk, like `prefetch` (see `load`).
fn prefetch_paths(memo: &mut Memo, paths: &[(String, Lang)]) {
    let mut seen = std::collections::HashSet::new();
    let todo: Vec<&(String, Lang)> = paths
        .iter()
        .filter(|(p, _)| !memo.contains_key(p) && seen.insert(p.as_str()))
        .collect();
    let done = par_analyse(&todo, |(p, l)| {
        let a = std::fs::read_to_string(p)
            .ok()
            .and_then(|src| calls::analyze(*l, &src).map(|(o, c)| (o, c, src)));
        (p.clone(), a)
    });
    memo.extend(done);
}

/// One parse per file for the whole run: a file that several levels hit is analysed once.
/// Serial fallback for anything `prefetch` did not cover.
fn analysed<'m>(memo: &'m mut Memo, f: &FileHits) -> Option<&'m Analyzed> {
    memo.entry(f.path.clone())
        .or_insert_with(|| {
            f.lang
                .and_then(|l| calls::analyze(l, &f.source))
                .map(|(o, c)| (o, c, f.source.clone()))
        })
        .as_ref()
}

/// Analyses a file that no search hit reached, reading it from disk and memoising it the
/// same way.
fn load<'m>(memo: &'m mut Memo, path: &str, lang: Lang) -> Option<&'m Analyzed> {
    memo.entry(path.to_owned())
        .or_insert_with(|| {
            let src = std::fs::read_to_string(path).ok()?;
            calls::analyze(lang, &src).map(|(o, c)| (o, c, src))
        })
        .as_ref()
}

/// Identity of a symbol within one walk: its file and start byte.
fn key(n: &Node) -> (String, usize) {
    (n.path.clone(), n.sym.start_byte)
}

/// The index of `node` in `nodes`, added when its symbol is new; the flag says it was added.
fn intern(
    nodes: &mut Vec<Node>,
    index: &mut HashMap<(String, usize), usize>,
    node: Node,
) -> (usize, bool) {
    let k = key(&node);
    if let Some(&i) = index.get(&k) {
        return (i, false);
    }
    nodes.push(node);
    index.insert(k, nodes.len() - 1);
    (nodes.len() - 1, true)
}

/// Stores one level's edges. A sure edge to a symbol not seen before gets its node, and that
/// node joins the next frontier; the returned indices are that frontier.
fn settle(
    level: Vec<(Edge, Option<Node>)>,
    edges: &mut Vec<Edge>,
    nodes: &mut Vec<Node>,
    index: &mut HashMap<(String, usize), usize>,
) -> Vec<usize> {
    let mut next = Vec::new();
    for (mut e, caller) in level {
        if let (true, Some(n)) = (e.sure, caller) {
            let (i, fresh) = intern(nodes, index, n);
            e.child = Some(i);
            if fresh {
                next.push(i);
            }
        }
        edges.push(e);
    }
    next
}

/// Whether a call of this form can reach a symbol of this kind. Rust keeps `macro_rules!` and
/// fns in separate namespaces: `name!()` reaches only a macro, and any other call no macro.
fn call_reaches(form: &CallForm, lang: Lang, kind: &str) -> bool {
    !matches!(lang, Lang::Rust) || (kind == "macro") == (*form == CallForm::Macro)
}

/// One call edge, printed under the node it hangs from. `addr` is the address printed for the
/// other end: the caller for callers, the callee definition for callees.
pub struct Edge {
    pub parent: usize,
    pub addr: String,
    pub lines: Vec<usize>,
    pub text: String,
    pub sure: bool,
    /// The node a sure edge leads to; unsure edges are printed but never expanded.
    pub child: Option<usize>,
}
