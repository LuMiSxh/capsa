// SPDX-License-Identifier: MPL-2.0
//! Owns the PowerShell segmentation on tree-sitter-powershell: one segment per `command` node,
//! with the stdin, stdout, nesting and exit-status gating of the nodes around it, as `bash` does
//! for Bash. A pipeline stage that is an expression (`(Get-Content a) -replace 'x','y' | sc a`) is
//! no segment, but the commands inside its parentheses are.

use super::climb::{Climber, Ctx, Role, climb, commands};
use super::*;
use tree_sitter::Node;

/// Reports whether `head` (lowercased) is a cmdlet, or the alias `sc` or `ac`, whose input goes to
/// a file.
fn is_sink(head: &str) -> bool {
    matches!(
        head,
        "out-file" | "set-content" | "sc" | "add-content" | "ac"
    )
}

/// Returns the segments of `src`, or `None` if it has a syntax error, in which case the caller
/// falls back to the hand-written splitter.
pub(super) fn parse(src: &str) -> Option<Parsed> {
    let tree = parse_tree(src, &tree_sitter_powershell::LANGUAGE.into())?;
    if tree.root_node().has_error() {
        return None;
    }
    let mut found: Vec<(Segment, bool)> =
        commands(&tree)?.into_iter().map(|n| one(src, n)).collect();
    // Commands inside `( )`, `$( )` and script blocks go last, so the stages of a pipeline stay
    // neighbours when one of them holds such a command.
    found.sort_by_key(|f| f.1);
    Some(Parsed {
        segs: found.into_iter().map(|f| f.0).collect(),
        docs: Vec::new(),
    })
}

/// Lists the node kinds that hold a nested expression, which a command sees as one word.
fn mask_of(kind: &str) -> Option<&'static str> {
    match kind {
        "sub_expression" => Some("$()"),
        "array_expression" => Some("@()"),
        "parenthesized_expression" => Some("()"),
        "script_block_expression" => Some("{}"),
        _ => None,
    }
}

/// Returns where the segment of a command starts: past a call operator
/// (`& 'C:\tools\rg.exe' foo`), which the rules do not read.
fn start_of(n: Node) -> usize {
    let mut cur = n.walk();
    let first = n
        .children(&mut cur)
        .find(|c| c.kind() != "command_invokation_operator");
    first.map_or(n.start_byte(), |c| c.start_byte())
}

/// Returns the command's text with each nested expression as one word, because the commands in
/// it are segments of their own and a word of theirs is not an operand of this one.
fn masked(src: &str, n: Node) -> String {
    let mut subs = Vec::new();
    let mut todo = vec![n];
    while let Some(c) = todo.pop() {
        if c != n && mask_of(c.kind()).is_some() {
            subs.push(c);
        } else {
            let mut cur = c.walk();
            todo.extend(c.children(&mut cur));
        }
    }
    subs.sort_by_key(|s| s.start_byte());
    let start = start_of(n);
    let end = n.start_byte() + src[n.byte_range()].trim_end().len();
    let (mut out, mut at) = (String::new(), start);
    for s in subs {
        out.push_str(&src[at..s.start_byte()]);
        out.push_str(mask_of(s.kind()).unwrap_or_default());
        at = s.end_byte().min(end);
    }
    out.push_str(&src[at.min(end)..end]);
    out
}

/// Reports whether a redirect operator sends stdout to a file: `>`, `>>`, `*>`, `*>>`, `1>`,
/// `1>>`. The grammar reads `2>err.txt` and `>$null` (no space) as plain arguments, so those
/// come here too.
fn stdout_redirect(op: &str) -> bool {
    let Some(i) = op.find('>') else { return false };
    let rest = op[i..].trim_start_matches('>');
    matches!(&op[..i], "" | "*" | "1") && op.len() - i - rest.len() <= 2 && !rest.starts_with('&')
}

/// Reports whether the command redirects its stdout.
fn redirects_stdout(src: &str, n: Node) -> bool {
    let mut todo = vec![n];
    while let Some(c) = todo.pop() {
        match c.kind() {
            // Redirects inside a nested expression belong to the commands in it.
            k if c != n && mask_of(k).is_some() => {}
            "file_redirection_operator" => {
                if stdout_redirect(&src[c.byte_range()]) {
                    return true;
                }
            }
            "generic_token" if c.parent().is_some_and(|p| p.kind() == "command_elements") => {
                if stdout_redirect(&src[c.byte_range()]) {
                    return true;
                }
            }
            _ => {
                let mut cur = c.walk();
                todo.extend(c.children(&mut cur));
            }
        }
    }
    false
}

/// Returns the segment of one `command` node, and whether it sits inside a nested expression.
fn one(src: &str, n: Node) -> (Segment, bool) {
    let head = n
        .child_by_field_name("command_name")
        .or_else(|| n.named_child(0))
        .filter(|c| c.kind() == "command_name")
        .map(|c| src[c.byte_range()].to_lowercase());
    let sink = head.as_deref().is_some_and(is_sink);
    let start = Ctx {
        out: (redirects_stdout(src, n) || sink).then_some(Out::File),
        stdin: false,
        nest: Nest::Top,
        inside: false,
        gates: false,
    };
    let walk = &mut Walk {
        src,
        last_stage: true,
    };
    let c = climb(n, start, walk);
    let low = lower_string(masked(src, n));
    let seg = Segment {
        orig: distinct(&low, src[start_of(n)..n.end_byte()].trim_end()),
        low,
        range: 0..0,
        lead: 0,
        stdin: c.stdin,
        out: c.out.unwrap_or(Out::Agent),
        nest: c.nest,
        gates: c.gates,
        assigns: 0,
        heredoc: false,
    };
    (seg, c.inside)
}

/// Reads the pipeline chains around a PowerShell command.
struct Walk<'a> {
    src: &'a str,
    /// Turns false once a stage follows the command in a chain.
    last_stage: bool,
}

impl Climber for Walk<'_> {
    fn role(p: Node, _: Node) -> Role {
        match p.kind() {
            // What runs in these is data for the expression, not output.
            "sub_expression"
            | "array_expression"
            | "parenthesized_expression"
            | "assignment_expression" => Role::Capture {
                inside: true,
                nest: Nest::Top,
            },
            "function_statement" => Role::Capture {
                inside: false,
                nest: Nest::Group,
            },
            // `& { … }` runs the block here; a block passed as an argument (`ForEach-Object
            // { … }`) is data for the command.
            "script_block_expression" => {
                if p.parent().is_some_and(|q| q.kind() == "command_name_expr") {
                    Role::Nest(Nest::Group)
                } else {
                    Role::Capture {
                        inside: true,
                        nest: Nest::Top,
                    }
                }
            }
            "pipeline_chain" => Role::Chain,
            "foreach_statement" | "for_statement" | "while_statement" | "do_statement" => {
                Role::Nest(Nest::Loop)
            }
            "if_statement" | "switch_statement" | "try_statement" | "statement_block" => {
                Role::Nest(Nest::Group)
            }
            _ => Role::Plain,
        }
    }

    fn captured(out: Option<Out>) -> Option<Out> {
        out.or(Some(Out::Capture))
    }

    fn chain(&mut self, p: Node, cur: Node, c: &mut Ctx) {
        let mut kids = p.walk();
        let stages: Vec<Node> = p.children(&mut kids).filter(|k| k.is_named()).collect();
        if stages.first().map(|k| k.id()) != Some(cur.id()) {
            c.stdin = true;
        }
        if stages.last().map(|k| k.id()) != Some(cur.id()) {
            c.out.get_or_insert(Out::Pipe);
            self.last_stage = false;
        }
        // `&&` or `||` after the chain gates on its last stage's status.
        c.gates = self.last_stage
            && p.next_sibling().is_some_and(|t| {
                t.kind() == "pipeline_chain_tail"
                    && matches!(self.src[t.byte_range()].trim(), "&&" | "||")
            });
    }
}
