// SPDX-License-Identifier: MPL-2.0
//! Owns the Bash segmentation on tree-sitter-bash: one segment per `command` node, with the stdin,
//! stdout, nesting and exit-status gating that the nodes around it give it. The zsh grammar is a
//! fork of this one with the same node kinds for everything used here, so `zsh` calls `parse` with
//! its language. The few kinds only zsh produces (`repeat_statement`, `select_statement`) never
//! occur in a Bash tree.

use super::climb::{Climber, Ctx, Role, climb, commands};
use super::*;
use tree_sitter::{Language, Node};

/// Returns the segments of `src` in the grammar `lang`, or `None` if it has a syntax error
/// (zsh-only syntax such as `${(f)…}` in the Bash grammar), so the call runs unchanged rather than
/// on a guess.
pub(super) fn parse(src: &str, lang: &Language) -> Option<Parsed> {
    let tree = parse_tree(src, lang)?;
    // `has_error` also covers MISSING nodes, which the parser inserts to repair the input.
    if tree.root_node().has_error() {
        return None;
    }
    let mut found: Vec<Found> = commands(&tree)?.into_iter().map(|n| one(src, n)).collect();
    // The commands inside substitutions go last, so the segments of a pipeline stay neighbours
    // when a stage holds one: `cat $(f) | tee >(g)` is `cat`, `tee`, `f`, `g`.
    found.sort_by_key(|f| f.inside);
    let mut docs = Vec::new();
    for (i, f) in found.iter().enumerate() {
        let Some(body) = &f.body else { continue };
        let mut openers = vec![f.seg.low.clone()];
        // `cat <<EOF | python` hands the body to `cat`, but `python` is what the line runs.
        if let Some(next) = found.get(i + 1).filter(|n| f.seg.feeds(&n.seg)) {
            openers.push(next.seg.low.clone());
        }
        docs.push(Doc {
            openers,
            body: body.clone(),
        });
    }
    Some(Parsed {
        segs: found.into_iter().map(|f| f.seg).collect(),
        docs,
    })
}

struct Found {
    seg: Segment,
    body: Option<String>,
    /// Reports whether it sits inside a substitution.
    inside: bool,
}

/// Returns `src[range]` as the rules read it: each substitution inside is one word, `$()` for
/// `$( )` and backticks and its first character (`<` or `>`) for `<( )` and `>( )`, because the
/// commands in it are segments of their own and a word of theirs is not an operand of this one.
fn masked(src: &str, owner: Node, range: &std::ops::Range<usize>) -> String {
    let mut subs = Vec::new();
    let mut todo = vec![owner];
    while let Some(n) = todo.pop() {
        if !matches!(n.kind(), "command_substitution" | "process_substitution") {
            let mut cur = n.walk();
            todo.extend(n.children(&mut cur).filter(|c| c.start_byte() < range.end));
        } else if n.start_byte() >= range.start && n.end_byte() <= range.end {
            subs.push(n);
        }
    }
    subs.sort_by_key(|n| n.start_byte());
    let (mut out, mut at) = (String::new(), range.start);
    for s in subs {
        out.push_str(&src[at..s.start_byte()]);
        out.push_str(if s.kind() == "command_substitution" {
            "$()"
        } else {
            &src[s.start_byte()..s.start_byte() + 1]
        });
        at = s.end_byte();
    }
    out.push_str(&src[at..range.end]);
    out
}

/// Returns what the redirects of a command say.
#[derive(Default)]
struct Redirs {
    stdout: bool,
    stdin: bool,
    /// A pipeline continues after a heredoc operator: `cat <<EOF | python`.
    pipes: bool,
    /// So does `&&` or `||`.
    gates: bool,
    /// The end of the last redirect, which the segment's range reaches.
    end: usize,
    body: Option<String>,
}

fn scan(node: Node, src: &str, r: &mut Redirs) {
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        match c.kind() {
            "file_redirect" => {
                r.stdout |= redirects_stdout_node(c, src);
                r.end = r.end.max(c.end_byte());
            }
            "herestring_redirect" => {
                r.stdin = true;
                r.end = r.end.max(c.end_byte());
            }
            "heredoc_redirect" => {
                r.stdin = true;
                let mut inner = c.walk();
                for h in c.children(&mut inner) {
                    match h.kind() {
                        "heredoc_start" => r.end = r.end.max(h.end_byte()),
                        "file_redirect" => {
                            r.stdout |= redirects_stdout_node(h, src);
                            r.end = r.end.max(h.end_byte());
                        }
                        "pipeline" => r.pipes = true,
                        "&&" | "||" => r.gates = true,
                        "heredoc_body" => r.body = Some(src[h.byte_range()].to_owned()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reports whether the redirect sends stdout somewhere: no descriptor or `1`, with `>`, `>>`,
/// `&>`, `&>>`, `>|` or `>&`.
fn redirects_stdout_node(redirect: Node, src: &str) -> bool {
    let mut cur = redirect.walk();
    let (mut fd, mut op) = (None, None);
    for c in redirect.children(&mut cur) {
        if c.kind() == "file_descriptor" {
            fd = Some(&src[c.byte_range()]);
        } else if !c.is_named() && op.is_none() {
            op = Some(c.kind());
        }
    }
    matches!(fd, None | Some("1")) && matches!(op, Some(">" | ">>" | "&>" | "&>>" | ">|" | ">&"))
}

fn is_body(statement: Node, node: Node) -> bool {
    statement.child_by_field_name("body").map(|b| b.id()) == Some(node.id())
}

/// Returns the segment of one `command` node, and the body of a heredoc it opens.
fn one(src: &str, n: Node) -> Found {
    // `cat a 2>&1` is a `redirected_statement` around the command; the redirects belong to it.
    let owner = n
        .parent()
        .filter(|p| p.kind() == "redirected_statement" && is_body(*p, n))
        .unwrap_or(n);
    let mut r = Redirs {
        end: n.end_byte(),
        ..Redirs::default()
    };
    scan(n, src, &mut r);
    if owner != n {
        scan(owner, src, &mut r);
    }
    let out = if r.stdout {
        Some(Out::File)
    } else if r.pipes {
        Some(Out::Pipe)
    } else {
        None
    };
    let start = Ctx {
        out,
        stdin: r.stdin,
        nest: Nest::Top,
        inside: false,
        gates: false,
    };
    let walk = &mut Walk {
        src,
        owner,
        r: &mut r,
        at_first: true,
    };
    let c = climb(owner, start, walk);

    let range = owner.start_byte()..r.end;
    let mut kids = n.walk();
    let kids: Vec<Node> = n.children(&mut kids).collect();
    let assigns = kids
        .iter()
        .take_while(|c| c.kind() == "variable_assignment")
        .count();
    let lead = kids
        .get(assigns)
        .map_or(0, |c| c.start_byte().saturating_sub(range.start));
    let low = quote_escaped_spaces(&masked(src, owner, &range));
    let seg = Segment {
        orig: distinct(&low, &src[range.clone()]),
        low,
        range,
        lead,
        stdin: c.stdin,
        out: c.out.unwrap_or(Out::Agent),
        nest: c.nest,
        gates: c.gates || r.gates || gates(owner),
        assigns,
        heredoc: r.body.is_some(),
    };
    Found {
        seg,
        body: r.body,
        inside: c.inside,
    }
}

/// Reads the lists, pipelines and redirects around a Bash command.
struct Walk<'a> {
    src: &'a str,
    owner: Node<'a>,
    r: &'a mut Redirs,
    /// Turns false once the path from the command runs through a later element of a list.
    at_first: bool,
}

impl Climber for Walk<'_> {
    fn role(p: Node, cur: Node) -> Role {
        match p.kind() {
            // What runs in these is data for the substitution or the function, not output.
            "command_substitution" | "process_substitution" => Role::Capture {
                inside: true,
                nest: Nest::Top,
            },
            "function_definition" => Role::Capture {
                inside: false,
                nest: Nest::Group,
            },
            "redirected_statement" if is_body(p, cur) => Role::Chain,
            "list" | "pipeline" => Role::Chain,
            "for_statement"
            | "c_style_for_statement"
            | "while_statement"
            | "repeat_statement"
            | "select_statement" => Role::Nest(Nest::Loop),
            "compound_statement" | "subshell" | "if_statement" | "case_statement" => {
                Role::Nest(Nest::Group)
            }
            _ => Role::Plain,
        }
    }

    /// A redirect inside `$( )` makes the output data for the substitution too.
    fn captured(out: Option<Out>) -> Option<Out> {
        Some(out.filter(|o| *o != Out::File).unwrap_or(Out::Capture))
    }

    fn chain(&mut self, p: Node, cur: Node, c: &mut Ctx) {
        let (src, owner, r) = (self.src, self.owner, &mut *self.r);
        match p.kind() {
            "redirected_statement" => {
                // The grammar wraps `a && b > f` and `a | b 2>&1` as a whole in the redirect,
                // but the shell applies it to the last command only.
                let chain = matches!(cur.kind(), "list" | "pipeline");
                // A chain takes the redirect as its last command, and a group there owns `owner`
                // too.
                let last = cur.child(cur.child_count().saturating_sub(1));
                let last_owns = last.is_some_and(|l| {
                    l.start_byte() <= owner.start_byte() && owner.end_byte() <= l.end_byte()
                });
                if !chain || last_owns {
                    let mut rr = Redirs::default();
                    scan(p, src, &mut rr);
                    if rr.stdout {
                        c.out.get_or_insert(Out::File);
                    }
                    if chain && rr.pipes {
                        c.out.get_or_insert(Out::Pipe);
                    }
                    c.stdin |= rr.stdin;
                    if chain {
                        r.end = r.end.max(rr.end);
                        r.body = r.body.take().or(rr.body);
                    }
                }
            }
            // The grammar reads `cmd <<EOF | a && b` as a pipeline of the list `a && b`; only `a` is
            // piped.
            "list" if p.child(0).map(|c| c.id()) != Some(cur.id()) => self.at_first = false,
            "pipeline" if cur.kind() == "list" && !self.at_first => {}
            "pipeline" => {
                if p.child(0).map(|c| c.id()) != Some(cur.id()) {
                    c.stdin = true;
                }
                if p.child(p.child_count().saturating_sub(1)).map(|c| c.id()) != Some(cur.id()) {
                    c.out.get_or_insert(Out::Pipe);
                }
            }
            _ => {}
        }
    }
}

/// Reports whether `&&` or `||` follows the node, directly or after the commands that end the list
/// it sits in (`cd x && cat a || echo`: `cat` gates the `||`).
fn gates(owner: Node) -> bool {
    let mut cur = owner;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "list" => {
                if p.child(0).map(|c| c.id()) == Some(cur.id()) {
                    return matches!(p.child(1).map(|o| o.kind()), Some("&&" | "||"));
                }
            }
            "negated_command" | "redirected_statement" | "heredoc_redirect" => {}
            // Only the last stage's status is the pipeline's.
            "pipeline" => {
                if p.child(p.child_count().saturating_sub(1)).map(|c| c.id()) != Some(cur.id()) {
                    return false;
                }
            }
            _ => return false,
        }
        cur = p;
    }
    false
}
