// SPDX-License-Identifier: MPL-2.0
//! `acus follow`: the callers or callees of a symbol, as a tree of call edges.

use crate::fmt::Out;
use acus_search::follow::{CALLABLE, Edge, Node, addr_of, callees, callers, container_of};
use acus_syntax::calls;
use acus_syntax::{Address, Lang, Resolve, Target, resolve};
use anyhow::Result;
use capsa_core::exit::Outcome;
use capsa_core::format::Format;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// path#Sym, as in `acus show`: the symbol whose callers are listed.
    #[arg(value_parser = crate::home::<String>)]
    addr: String,
    /// How many levels to follow (1 = direct callers or callees only).
    #[arg(short = 'd', long, default_value_t = 2)]
    depth: usize,
    /// List callees (what the symbol calls) instead of callers.
    #[arg(long)]
    down: bool,
    /// Lift the output caps: every edge and every unsure call is printed.
    #[arg(long)]
    all: bool,
}

/// Sure edges printed per node, and unsure ones; more than this collapse unless `--all`.
const SURE_CAP: usize = 10;
const UNSURE_CAP: usize = 15;
/// Text lines before the walk stops expanding, unless `--all`.
const LINE_BUDGET: usize = 40;

#[expect(
    clippy::needless_pass_by_value,
    reason = "execute hands over the parsed arguments"
)]
pub fn run(a: Args, out: &Out) -> Result<Outcome> {
    let max = a.depth.max(1);
    let dir = if a.down { "callees" } else { "callers" };
    let mut nodes = vec![resolve_target(&a.addr)?];
    let (edges, unresolved) = if a.down {
        callees(&mut nodes, max, a.all)?
    } else {
        (callers(&mut nodes, max)?, HashMap::new())
    };
    let root = &nodes[0];
    let mut tree = Tree {
        nodes: &nodes,
        edges: &edges,
        callers: !a.down,
        all: a.all,
        max,
        sure_parents: edges
            .iter()
            .filter(|e| e.sure && e.child.is_some())
            .map(|e| e.parent)
            .collect(),
        unresolved,
        printed: HashSet::from([0]),
        lines: Vec::new(),
        shown: Vec::new(),
        more: Vec::new(),
        stopped: false,
    };
    tree.node(0, 1);
    if tree.stopped {
        tree.lines
            .push(format!("… more: acus follow {} --all", root.addr));
    }
    if out.format == Format::Json {
        let edge = |&(i, depth): &(usize, usize)| {
            let e = &edges[i];
            let parent = &nodes[e.parent].addr;
            let (from, to) = if a.down {
                (parent.as_str(), e.addr.as_str())
            } else {
                (e.addr.as_str(), parent.as_str())
            };
            json!({
                "from": from, "to": to, "lines": e.lines, "text": e.text,
                "sure": e.sure, "depth": depth,
            })
        };
        let more: Vec<_> = tree
            .more
            .iter()
            .map(|(at, n, next)| match n {
                Some(n) => json!({"at": at, "more": n, "next": next}),
                None => json!({"at": at, "next": next}),
            })
            .collect();
        println!(
            "{}",
            json!({
                "target": {"addr": root.addr, "kind": root.sym.kind,
                           "start": root.sym.start_line, "end": root.sym.end_line},
                "direction": dir,
                "edges": tree.shown.iter().map(edge).collect::<Vec<_>>(),
                "truncated": more,
            })
        );
    } else {
        let header = format!(
            "{}  {} {}-{}  {dir} depth {max}",
            root.addr, root.sym.kind, root.sym.start_line, root.sym.end_line
        );
        println!("{}", out.header(&header));
        for l in &tree.lines {
            println!("{l}");
        }
    }
    Ok(if edges.is_empty() {
        Outcome::Empty
    } else {
        Outcome::Found
    })
}

/// The rows `follow` prints, in tree order, with the caps applied. Text and JSON both come
/// from one walk, so they cannot disagree about what was cut.
struct Tree<'a> {
    nodes: &'a [Node],
    edges: &'a [Edge],
    callers: bool,
    all: bool,
    /// The deepest level printed; edges at this level may end in a depth-limit line.
    max: usize,
    /// Nodes with a sure edge of their own: only those get a depth-limit line.
    sure_parents: HashSet<usize>,
    /// Unsure method calls per node that `--down` left out of the edges.
    unresolved: HashMap<usize, usize>,
    /// Nodes expanded so far, in print order: a later copy is marked `(seen above)`.
    printed: HashSet<usize>,
    lines: Vec<String>,
    /// `(edge index, level)` printed, in order (the JSON `edges`).
    shown: Vec<(usize, usize)>,
    /// `(at, more, next)` per truncation; `more` is absent for a depth limit (the JSON
    /// `truncated`).
    more: Vec<(String, Option<usize>, String)>,
    stopped: bool,
}

impl Tree<'_> {
    /// The edges under node `idx`; `depth` is their level, which sets the indent.
    fn node(&mut self, idx: usize, depth: usize) {
        let edges = self.edges;
        let at = self.nodes[idx].addr.clone();
        let ind = "  ".repeat(depth);
        let (mut n_sure, mut n_unsure, mut hidden_sure) = (0, 0, 0);
        let mut hidden_unsure: Vec<&str> = Vec::new();
        for i in (0..edges.len()).filter(|&i| edges[i].parent == idx) {
            let e = &edges[i];
            let over = if e.sure {
                n_sure += 1;
                n_sure > SURE_CAP
            } else {
                n_unsure += 1;
                n_unsure > UNSURE_CAP
            };
            if over && !self.all {
                if e.sure {
                    hidden_sure += 1;
                } else {
                    hidden_unsure.push(e.addr.split(['#', ':']).next().unwrap_or(""));
                }
                continue;
            }
            self.edge(i, depth);
        }
        if hidden_sure > 0 {
            let next = format!("acus follow {at} --all");
            self.lines
                .push(format!("{ind}… {hidden_sure} more: {next}"));
            self.more.push((at.clone(), Some(hidden_sure), next));
        }
        if !hidden_unsure.is_empty() {
            let mut files = hidden_unsure.clone();
            files.sort_unstable();
            files.dedup();
            let head = if self.callers {
                format!(".{}() ", self.nodes[idx].name)
            } else {
                String::new()
            };
            let n = hidden_unsure.len();
            let next = format!("acus follow {at} --all");
            let unit = if files.len() == 1 { "file" } else { "files" };
            self.lines.push(format!(
                "{ind}? {n} more {head}calls in {} {unit}: {next}",
                files.len()
            ));
            self.more.push((at.clone(), Some(n), next));
        }
        // In `--down` without `--all`, unsure method calls are no edges: one line counts them.
        if let Some(&n) = self.unresolved.get(&idx) {
            let next = format!("acus follow {at} --down --all");
            self.lines
                .push(format!("{ind}? {n} unresolved method calls: {next}"));
            self.more.push((at, Some(n), next));
        }
    }

    fn edge(&mut self, i: usize, depth: usize) {
        let edges = self.edges;
        let e = &edges[i];
        if !self.all && self.lines.len() >= LINE_BUDGET {
            self.stopped = true;
            return;
        }
        let ind = "  ".repeat(depth);
        let lines: Vec<String> = e.lines.iter().map(usize::to_string).collect();
        let mark = if e.sure { "" } else { "?  " };
        // A node is expanded where it is first printed; a later copy only points back.
        let seen = e.child.is_some_and(|c| self.printed.contains(&c));
        let seen_mark = if seen { "  (seen above)" } else { "" };
        self.lines.push(format!(
            "{ind}{}  {}  {mark}{}{seen_mark}",
            e.addr,
            lines.join(","),
            e.text
        ));
        self.shown.push((i, depth));
        let Some(c) = e.child.filter(|_| !seen) else {
            return;
        };
        if depth < self.max {
            self.printed.insert(c);
            self.node(c, depth + 1);
        } else if self.sure_parents.contains(&c) {
            let next = format!("acus follow {}", e.addr);
            self.lines.push(format!("{ind}  … depth limit: {next}"));
            self.more.push((e.addr.clone(), None, next));
        }
    }
}

/// Resolves `path#Sym` to a callable symbol; anything else exits 2 with the supported kinds.
fn resolve_target(raw: &str) -> Result<Node> {
    let addr = Address::parse(raw);
    let Target::Symbol(q) = &addr.target else {
        return Err(capsa_core::hint::Hinted::new(
            "follow needs path#Sym, not a line range",
            format!("acus follow {}#Symbol", addr.path),
        )
        .into());
    };
    let path = addr.path.as_str();
    let src = std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => anyhow::anyhow!("path not found: {path}"),
        _ => anyhow::anyhow!("{path}: {e}"),
    })?;
    let lang = Lang::from_path(Path::new(path));
    let supported = lang.is_some_and(calls::supports_calls);
    let Some((outline, _)) = lang
        .filter(|_| supported)
        .and_then(|l| calls::analyze(l, &src))
    else {
        return Err(capsa_core::hint::Hinted::new(
            "acus follow supports Rust, Swift, TypeScript, JavaScript, Python, C and Shell",
            format!("{path} has no call rules; use acus outline {path} to find a function"),
        )
        .into());
    };
    let sym = match resolve(&outline, q) {
        Resolve::Found(s) => s.clone(),
        Resolve::Missing => {
            return Err(capsa_core::hint::Hinted::new(
                format!("no symbol `{q}` in {path}"),
                format!("acus outline {path}"),
            )
            .into());
        }
        Resolve::Ambiguous(v) => {
            let c: Vec<_> = v
                .iter()
                .map(|s| format!("{path}:{}-{} ({})", s.start_line, s.end_line, s.qual))
                .collect();
            return Err(capsa_core::hint::Hinted::new(
                format!("`{q}` is ambiguous in {path}"),
                c.join(", "),
            )
            .into());
        }
    };
    if !CALLABLE.contains(&sym.kind) {
        return Err(capsa_core::hint::Hinted::new(
            format!("`{}` is a {}, not callable", sym.qual, sym.kind),
            format!("acus follow works on {}", CALLABLE.join(", ")),
        )
        .into());
    }
    let parent = container_of(&sym.qual);
    let ctor = matches!(sym.name.as_str(), "init" | "__init__" | "constructor");
    let (name, container) = if ctor {
        (parent.clone().unwrap_or_else(|| sym.name.clone()), parent)
    } else {
        (sym.name.clone(), parent)
    };
    let addr = addr_of(path, &outline, &sym);
    Ok(Node {
        path: path.to_owned(),
        addr,
        // Supported above, so the extension has a grammar.
        lang: lang.unwrap_or(Lang::Rust),
        sym,
        name,
        container,
    })
}
