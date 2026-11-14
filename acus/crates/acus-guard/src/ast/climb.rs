// SPDX-License-Identifier: MPL-2.0
//! Owns the walk from a `command` node up to the root that both tree-sitter backends make: what the
//! nodes around a command say about where its stdout goes, whether it reads a pipe, how deep it is
//! nested and whether `&&` or `||` follows. Each dialect says what its node kinds mean; the rules
//! for applying that exist here once.

use super::*;
use tree_sitter::{Node, Tree};

/// Limits the nesting of a command an agent would write; deeper, the call runs unchanged.
const MAX_DEPTH: usize = 400;

/// Returns the `command` nodes of `tree` in source order, or `None` when they nest deeper than
/// `MAX_DEPTH`.
pub(super) fn commands(tree: &Tree) -> Option<Vec<Node<'_>>> {
    let mut commands = Vec::new();
    let mut cur = tree.walk();
    loop {
        if cur.node().kind() == "command" {
            commands.push(cur.node());
        }
        if cur.depth() as usize > MAX_DEPTH {
            return None;
        }
        if cur.goto_first_child() {
            continue;
        }
        while !cur.goto_next_sibling() {
            if !cur.goto_parent() {
                return Some(commands);
            }
        }
    }
}

/// Holds what the nodes around a command have said about its segment so far.
pub(super) struct Ctx {
    pub(super) out: Option<Out>,
    pub(super) stdin: bool,
    pub(super) nest: Nest,
    /// Whether it sits inside a substitution or nested expression.
    pub(super) inside: bool,
    /// Whether `&&` or `||` follows, for a dialect whose chain node says so.
    pub(super) gates: bool,
}

/// Says what a parent node does to the segments below it.
pub(super) enum Role {
    /// What runs in it is data for the node, not output, and the walk ends. `inside` marks a
    /// substitution, `nest` the depth of a body that runs elsewhere (a function).
    Capture { inside: bool, nest: Nest },
    /// A compound command: the segment sits at least this deep.
    Nest(Nest),
    /// A node whose children are positions (list, pipeline, redirect): `Climber::chain` reads it.
    Chain,
    /// Nothing about the segment.
    Plain,
}

/// Holds one dialect's reading of its node kinds.
pub(super) trait Climber {
    /// Returns what the parent `p` of `cur` is.
    fn role(p: Node, cur: Node) -> Role;

    /// Returns the output of a segment in a `Role::Capture` node, given what it had before.
    fn captured(out: Option<Out>) -> Option<Out>;

    /// Applies the list, pipeline or redirect `p` that `cur` sits in.
    fn chain(&mut self, p: Node, cur: Node, ctx: &mut Ctx);
}

/// Climbs from `owner` to the root, applying what each node on the way says about the segment.
pub(super) fn climb<C: Climber>(owner: Node, mut ctx: Ctx, dialect: &mut C) -> Ctx {
    let mut cur = owner;
    while let Some(p) = cur.parent() {
        match C::role(p, cur) {
            Role::Capture { inside, nest } => {
                ctx.out = C::captured(ctx.out);
                ctx.inside |= inside;
                ctx.nest = ctx.nest.max(nest);
                break;
            }
            Role::Nest(nest) => ctx.nest = ctx.nest.max(nest),
            Role::Chain => dialect.chain(p, cur, &mut ctx),
            Role::Plain => {}
        }
        cur = p;
    }
    ctx
}
