// SPDX-License-Identifier: MPL-2.0
//! Callees of a symbol: the calls inside it, resolved to the definitions in the repo.

use super::{
    CALLABLE, Edge, Memo, Node, addr_of, analysed, call_reaches, container_of, key, load, prefetch,
    prefetch_paths, search, settle,
};
use crate::{FileHits, FindError};
use acus_syntax::calls::{Call, CallForm};
use acus_syntax::{Lang, Symbol};
use std::collections::HashMap;
use std::path::Path;

/// A definition that a callee name can resolve to, with what its edge prints.
struct Def {
    path: String,
    sym: Symbol,
    addr: String,
    lang: Lang,
}

/// Returns the callees of `nodes[0]`, up to `max` levels: the calls inside each frontier
/// symbol, resolved to definitions in the repo. Calls with no repo definition (std, external)
/// are dropped. Unless `all`, a method call with an unknown receiver is no edge: the count per
/// caller is returned.
pub fn callees(
    nodes: &mut Vec<Node>,
    max: usize,
    all: bool,
) -> Result<(Vec<Edge>, HashMap<usize, usize>), FindError> {
    let mut memo = HashMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut unresolved: HashMap<usize, usize> = HashMap::new();
    let mut index = HashMap::from([(key(&nodes[0]), 0)]);
    let mut frontier = vec![0];
    for depth in 1..=max + 1 {
        let work = frontier_calls(&mut memo, nodes, &frontier);
        let mut names: Vec<String> = work.iter().map(|(_, c, _)| c.name.clone()).collect();
        names.sort();
        names.dedup();
        if names.is_empty() {
            break;
        }
        let lang = nodes[0].lang;
        let files = search(&names, lang)?;
        prefetch(&mut memo, &files);
        let defs = definitions(&mut memo, &files, lang);
        let level = resolve_level(&work, nodes, &defs, all, &mut unresolved);
        let next = settle(level, &mut edges, nodes, &mut index);
        // The level past the limit is only probed for sure edges, so it is not expanded.
        if depth > max || next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok((edges, unresolved))
}

/// The calls inside each frontier symbol, each with the source line it prints.
fn frontier_calls(
    memo: &mut Memo,
    nodes: &[Node],
    frontier: &[usize],
) -> Vec<(usize, Call, String)> {
    let paths: Vec<(String, Lang)> = frontier
        .iter()
        .map(|&fi| (nodes[fi].path.clone(), nodes[fi].lang))
        .collect();
    prefetch_paths(memo, &paths);
    let mut work = Vec::new();
    for &fi in frontier {
        let n = &nodes[fi];
        let Some((_, sites, src)) = load(memo, &n.path, n.lang) else {
            continue;
        };
        for c in sites
            .iter()
            .filter(|c| (n.sym.start_byte..n.sym.end_byte).contains(&c.byte))
        {
            let text = src.lines().nth(c.line - 1).unwrap_or("").trim().to_owned();
            work.push((fi, c.clone(), text));
        }
    }
    work
}

/// The callable definitions in `files`, grouped by name.
fn definitions(memo: &mut Memo, files: &[FileHits], lang: Lang) -> HashMap<String, Vec<Def>> {
    let mut defs: HashMap<String, Vec<Def>> = HashMap::new();
    for f in files {
        let Some((o, _, _)) = analysed(memo, f) else {
            continue;
        };
        for s in o.symbols.iter().filter(|s| CALLABLE.contains(&s.kind)) {
            defs.entry(s.name.clone()).or_default().push(Def {
                path: f.path.clone(),
                sym: s.clone(),
                addr: addr_of(&f.path, o, s),
                lang: f.lang.unwrap_or(lang),
            });
        }
    }
    defs
}

/// The edges one frontier level adds, each with the caller node it leads to.
fn resolve_level(
    work: &[(usize, Call, String)],
    nodes: &[Node],
    defs: &HashMap<String, Vec<Def>>,
    all: bool,
    unresolved: &mut HashMap<usize, usize>,
) -> Vec<(Edge, Option<Node>)> {
    let mut level: Vec<(Edge, Option<Node>)> = Vec::new();
    for (fi, c, text) in work {
        let t = &nodes[*fi];
        let Some(named) = defs.get(&c.name) else {
            continue;
        };
        let cands: Vec<&Def> = named
            .iter()
            .filter(|d| call_reaches(&c.form, d.lang, d.sym.kind))
            .collect();
        let (picks, sure): (Vec<&Def>, bool) = match c.form {
            // Unknown receivers (`x.f()`) only match by name: kept for `--all`, else counted.
            CallForm::Method if !all => {
                if !cands.is_empty() {
                    *unresolved.entry(*fi).or_default() += 1;
                }
                continue;
            }
            CallForm::Method => (cands.clone(), false),
            _ => match if c.form == CallForm::SelfRecv {
                t.container.clone()
            } else {
                c.qual.clone()
            } {
                Some(q) => {
                    let hit: Vec<&Def> = cands
                        .iter()
                        .copied()
                        .filter(|d| {
                            container_of(&d.sym.qual).as_deref() == Some(q.as_str())
                                || module_is(d, &q)
                        })
                        .collect();
                    let one = hit.len() == 1;
                    (hit, one)
                }
                None => {
                    let in_file: Vec<&Def> =
                        cands.iter().copied().filter(|d| d.path == t.path).collect();
                    let pool = if in_file.is_empty() {
                        cands.clone()
                    } else {
                        in_file
                    };
                    let one = pool.len() == 1 && !c.in_macro;
                    (pool, one)
                }
            },
        };
        let Some(d) = picks.first() else {
            continue;
        };
        // A recursive call is the symbol calling itself: not a callee.
        if d.path == t.path && d.sym.start_byte == t.sym.start_byte {
            continue;
        }
        let caller = Node {
            path: d.path.clone(),
            addr: d.addr.clone(),
            sym: d.sym.clone(),
            name: d.sym.name.clone(),
            container: container_of(&d.sym.qual),
            lang: d.lang,
        };
        match level
            .iter_mut()
            .find(|(e, _)| e.parent == *fi && e.addr == d.addr)
        {
            Some((e, _)) => {
                e.lines.push(c.line);
                e.sure &= sure;
            }
            None => level.push((
                Edge {
                    parent: *fi,
                    addr: d.addr.clone(),
                    lines: vec![c.line],
                    text: text.clone(),
                    sure,
                    child: None,
                },
                Some(caller),
            )),
        }
    }
    level
}

/// Whether `d` sits in the module (file stem) `q`, for `module::func(..)` calls.
fn module_is(d: &Def, q: &str) -> bool {
    Path::new(&d.path).file_stem().and_then(|s| s.to_str()) == Some(q)
}
