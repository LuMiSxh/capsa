// SPDX-License-Identifier: MPL-2.0
//! Callers of a symbol: the files that mention its name, resolved to the calls that reach it.

use super::{
    Edge, Node, addr_of, analysed, call_reaches, container_of, key, prefetch, search, settle,
};
use acus_search::FindError;
use acus_syntax::calls::{Call, CallForm};
use std::collections::HashMap;

/// Returns the callers of `nodes[0]`, up to `max` levels. Each level searches every frontier
/// name in one pass; only sure edges create the next frontier.
pub fn callers(nodes: &mut Vec<Node>, max: usize) -> Result<Vec<Edge>, FindError> {
    let mut memo = HashMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut index = HashMap::from([(key(&nodes[0]), 0)]);
    let mut frontier = vec![0];
    for depth in 1..=max + 1 {
        let mut names: Vec<String> = frontier.iter().map(|&i| nodes[i].name.clone()).collect();
        names.sort();
        names.dedup();
        let files = search(&names, nodes[0].lang)?;
        // Every hit file is analysed before classifying, so the definition counts cover them all.
        prefetch(&mut memo, &files);
        let mut defs: HashMap<String, usize> = HashMap::new();
        for f in &files {
            if let Some((o, _, _)) = analysed(&mut memo, f) {
                for s in &o.symbols {
                    *defs.entry(s.name.clone()).or_default() += 1;
                }
            }
        }
        let mut level: Vec<(Edge, Option<Node>)> = Vec::new();
        for f in &files {
            let Some((o, sites, _)) = analysed(&mut memo, f) else {
                continue;
            };
            for c in sites.iter().filter(|c| names.contains(&c.name)) {
                let enc = o.enclosing_at(c.byte);
                let caller_container = enc.and_then(|s| container_of(&s.qual));
                for &fi in frontier.iter().filter(|&&i| {
                    nodes[i].name == c.name
                        && call_reaches(&c.form, nodes[i].lang, nodes[i].sym.kind)
                }) {
                    let t = &nodes[fi];
                    // A recursive call is the symbol calling itself: not a caller.
                    if enc.is_some_and(|s| f.path == t.path && s.start_byte == t.sym.start_byte) {
                        continue;
                    }
                    let same_file = f.path == t.path;
                    let defined = defs.get(&c.name).copied().unwrap_or(0);
                    let sure = sure_call(c, caller_container.as_deref(), t, same_file, defined);
                    let addr = match enc {
                        Some(s) => addr_of(&f.path, o, s),
                        None => format!("{}:{}", f.path, c.line),
                    };
                    match level
                        .iter_mut()
                        .find(|(e, _)| e.parent == fi && e.addr == addr)
                    {
                        Some((e, _)) => {
                            e.lines.push(c.line);
                            e.sure &= sure;
                        }
                        None => {
                            let caller = enc.map(|s| Node {
                                path: f.path.clone(),
                                addr: addr.clone(),
                                sym: s.clone(),
                                name: s.name.clone(),
                                container: container_of(&s.qual),
                                lang: f.lang.unwrap_or(nodes[0].lang),
                            });
                            let text = f.source.lines().nth(c.line - 1).unwrap_or("").trim();
                            let edge = Edge {
                                parent: fi,
                                addr,
                                lines: vec![c.line],
                                text: text.to_owned(),
                                sure,
                                child: None,
                            };
                            level.push((edge, caller));
                        }
                    }
                }
            }
        }
        let next = settle(level, &mut edges, nodes, &mut index);
        // The level past the limit is only probed for sure edges, so it is not expanded.
        if depth > max || next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(edges)
}

/// Whether a call site certainly reaches `target`. Uncertain (`?`) edges are printed but never
/// followed. `defs` counts the definitions of the call's name in the files the search hit.
fn sure_call(
    c: &Call,
    caller_container: Option<&str>,
    target: &Node,
    same_file: bool,
    defs: usize,
) -> bool {
    let target_container = target.container.as_deref();
    match c.form {
        CallForm::Path | CallForm::Ctor
            if c.qual.is_some() && c.qual.as_deref() == target_container =>
        {
            return true;
        }
        CallForm::SelfRecv
            if caller_container.is_some() && caller_container == target_container =>
        {
            return true;
        }
        _ => {}
    }
    // A `name!()` call can only reach a macro; a fn of the same name is a name match.
    if matches!(c.form, CallForm::Macro) && target.sym.kind != "macro" {
        return false;
    }
    // Macro arguments are token streams, so an unqualified name there may not be a call.
    if c.in_macro && c.qual.is_none() {
        return false;
    }
    match c.form {
        CallForm::Bare | CallForm::Macro | CallForm::Ctor => same_file || defs == 1,
        _ => false,
    }
}
