// SPDX-License-Identifier: MPL-2.0
//! Owns the call sites of a file: the names it calls and the form of each call. Reuses the
//! outline's parse, so `analyze` costs one tree-sitter parse. Svelte is outlined but has no call
//! rules, since its scripts are only parsed as TypeScript ranges.
use crate::Lang;
use crate::outline::{Outline, parse_outline};
use crate::rules::text;
use serde::Serialize;
use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Call {
    /// Names the callee: `parse` for `Parser::parse(x)`, `foo` for `foo!(x)`.
    pub name: String,
    /// Holds the last segment of the path qualifier, or a plain-word receiver (`Parser`, `self`).
    pub qual: Option<String>,
    pub form: CallForm,
    /// Holds the 1-based line of the callee name.
    pub line: usize,
    /// Holds the byte offset of the callee name, to tell apart calls that share a line.
    pub byte: usize,
    /// Set for a call read from a Rust macro's unparsed token tree, not from an expression.
    pub in_macro: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CallForm {
    /// `parse(x)`; also Python and Swift constructors `Foo(x)`.
    Bare,
    /// `Parser::parse(x)`, `module::func(x)`.
    Path,
    /// `self.parse()`, `this.parse()`, `Self::parse()`.
    SelfRecv,
    /// `x.parse()` with a receiver that is not `self`.
    Method,
    /// `new Foo()`, `Foo { .. }`, `<Foo />`.
    Ctor,
    /// Rust `foo!(...)`.
    Macro,
}

/// Reports whether a language has call rules; the others get an outline and no calls.
pub fn supports_calls(lang: Lang) -> bool {
    matches!(
        lang,
        Lang::Rust
            | Lang::Swift
            | Lang::TypeScript
            | Lang::Tsx
            | Lang::JavaScript
            | Lang::Python
            | Lang::C
            | Lang::Shell
    )
}

/// Returns the outline of `src` and its call sites in source order, or `None` when the grammar is
/// not compiled in.
pub fn analyze(lang: Lang, src: &str) -> Option<(Outline, Vec<Call>)> {
    let (outline, tree) = parse_outline(lang, src)?;
    let mut calls = Vec::new();
    if supports_calls(lang) {
        walk(lang, tree.root_node(), src.as_bytes(), false, &mut calls);
    }
    calls.sort_by_key(|c| c.line);
    Some((outline, calls))
}

fn walk(lang: Lang, node: Node, src: &[u8], in_macro: bool, out: &mut Vec<Call>) {
    let in_macro = in_macro || (lang == Lang::Rust && node.kind() == "token_tree");
    if lang == Lang::Rust && node.kind() == "token_tree" {
        rust_token_tree(node, src, out);
    }
    if let Some(c) = node_call(lang, node, src) {
        out.push(Call { in_macro, ..c });
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        walk(lang, child, src, in_macro, out);
    }
}

fn node_call(lang: Lang, n: Node, src: &[u8]) -> Option<Call> {
    match lang {
        Lang::Rust => rust(n, src),
        Lang::Python => python(n, src),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => ts(n, src),
        Lang::C => c(n, src),
        Lang::Swift => swift(n, src),
        Lang::Shell => shell(n, src),
        _ => None,
    }
}

fn call(name: Node, src: &[u8], qual: Option<String>, form: CallForm) -> Call {
    Call {
        name: text(name, src).to_owned(),
        qual,
        form,
        line: name.start_position().row + 1,
        byte: name.start_byte(),
        in_macro: false,
    }
}

/// Reports whether a node is a plain word, such as an identifier or `self`/`this`; not a call,
/// index or chain.
fn word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Sets the form of `recv.name`: a word receiver becomes `qual`; `selves` are the words that mean
/// the enclosing type and turn the form into `SelfRecv`.
fn receiver_call(recv: Option<Node>, name: Node, src: &[u8], selves: &[&str]) -> Call {
    let qual = recv
        .map(|r| text(r, src))
        .filter(|t| word(t))
        .map(str::to_owned);
    let form = match &qual {
        Some(q) if selves.contains(&q.as_str()) => CallForm::SelfRecv,
        _ => CallForm::Method,
    };
    call(name, src, qual, form)
}

fn named_child_of_kind<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cur = n.walk();
    n.named_children(&mut cur).find(|c| c.kind() == kind)
}

/// Returns the name of a Rust path `a::b::name`, with the text of the segment before it.
fn rust_path<'t>(n: Node<'t>, src: &[u8]) -> (Node<'t>, Option<String>) {
    match n.kind() {
        "scoped_identifier" | "scoped_type_identifier" => {
            let name = n.child_by_field_name("name").unwrap_or(n);
            let qual = n
                .child_by_field_name("path")
                .map(|p| text(rust_path(p, src).0, src).to_owned());
            (name, qual)
        }
        "generic_type" => match n.child_by_field_name("type") {
            Some(t) => rust_path(t, src),
            None => (n, None),
        },
        _ => (n, None),
    }
}

fn rust(n: Node, src: &[u8]) -> Option<Call> {
    match n.kind() {
        "call_expression" => {
            let mut f = n.child_by_field_name("function")?;
            if f.kind() == "generic_function" {
                f = f.child_by_field_name("function")?;
            }
            match f.kind() {
                "identifier" => Some(call(f, src, None, CallForm::Bare)),
                "scoped_identifier" => {
                    let (name, qual) = rust_path(f, src);
                    let form = if qual.as_deref() == Some("Self") {
                        CallForm::SelfRecv
                    } else {
                        CallForm::Path
                    };
                    Some(call(name, src, qual, form))
                }
                "field_expression" => {
                    let field = f.child_by_field_name("field")?;
                    if field.kind() != "field_identifier" {
                        return None;
                    }
                    Some(receiver_call(
                        f.child_by_field_name("value"),
                        field,
                        src,
                        &["self"],
                    ))
                }
                _ => None,
            }
        }
        "macro_invocation" => {
            let (name, qual) = rust_path(n.child_by_field_name("macro")?, src);
            Some(call(name, src, qual, CallForm::Macro))
        }
        "struct_expression" => {
            let (name, qual) = rust_path(n.child_by_field_name("name")?, src);
            Some(call(name, src, qual, CallForm::Ctor))
        }
        _ => None,
    }
}

/// Finds the calls in macro arguments, which are unparsed token trees: a name followed by a `(`
/// group is a call, `name!` a macro call, and a `::` or `.` before the name gives the form.
fn rust_token_tree(tt: Node, src: &[u8], out: &mut Vec<Call>) {
    let mut cur = tt.walk();
    let toks: Vec<Node> = tt.children(&mut cur).collect();
    let is_word = |n: Option<Node>| n.is_some_and(|n| matches!(n.kind(), "identifier" | "self"));
    for (i, &t) in toks.iter().enumerate() {
        if !matches!(t.kind(), "identifier" | "self") {
            continue;
        }
        let before = |k: usize| i.checked_sub(k).map(|j| toks[j]);
        let next = toks.get(i + 1).copied();
        let sep = before(1).map(|p| text(p, src));
        let (qual, form) = match sep {
            Some("::") => {
                let q = before(2)
                    .filter(|&r| is_word(Some(r)))
                    .map(|r| text(r, src).to_owned());
                let form = if q.as_deref() == Some("Self") {
                    CallForm::SelfRecv
                } else {
                    CallForm::Path
                };
                (q, form)
            }
            Some(".") => match before(2).filter(|&r| is_word(Some(r))) {
                Some(r) if text(r, src) == "self" => (Some("self".to_owned()), CallForm::SelfRecv),
                Some(r) if !matches!(before(3).map(|p| text(p, src)), Some("." | "::")) => {
                    (Some(text(r, src).to_owned()), CallForm::Method)
                }
                _ => (None, CallForm::Method),
            },
            _ => (None, CallForm::Bare),
        };
        let macro_bang = next.is_some_and(|n| text(n, src) == "!")
            && toks.get(i + 2).is_some_and(|n| n.kind() == "token_tree");
        if macro_bang {
            let q = if sep == Some("::") { qual } else { None };
            out.push(Call {
                in_macro: true,
                ..call(t, src, q, CallForm::Macro)
            });
        } else if next.is_some_and(|n| n.kind() == "token_tree" && text(n, src).starts_with('(')) {
            out.push(Call {
                in_macro: true,
                ..call(t, src, qual, form)
            });
        }
    }
}

fn python(n: Node, src: &[u8]) -> Option<Call> {
    match n.kind() {
        "call" => python_callee(n.child_by_field_name("function")?, src),
        // `@foo` and `@foo.bar` run the decorator as a call with no arguments.
        "decorator" => python_callee(n.named_child(0)?, src),
        _ => None,
    }
}

fn python_callee(f: Node, src: &[u8]) -> Option<Call> {
    match f.kind() {
        "identifier" => Some(call(f, src, None, CallForm::Bare)),
        "attribute" => Some(receiver_call(
            f.child_by_field_name("object"),
            f.child_by_field_name("attribute")?,
            src,
            &["self", "cls"],
        )),
        _ => None,
    }
}

fn ts(n: Node, src: &[u8]) -> Option<Call> {
    match n.kind() {
        "call_expression" => {
            let f = n.child_by_field_name("function")?;
            match f.kind() {
                "identifier" => Some(call(f, src, None, CallForm::Bare)),
                "member_expression" => Some(receiver_call(
                    f.child_by_field_name("object"),
                    f.child_by_field_name("property")?,
                    src,
                    &["this"],
                )),
                _ => None,
            }
        }
        "new_expression" => {
            let f = n.child_by_field_name("constructor")?;
            match f.kind() {
                "identifier" => Some(call(f, src, None, CallForm::Ctor)),
                "member_expression" => {
                    let name = f.child_by_field_name("property")?;
                    let r = receiver_call(f.child_by_field_name("object"), name, src, &[]);
                    Some(Call {
                        form: CallForm::Ctor,
                        ..r
                    })
                }
                _ => None,
            }
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            let name = n.child_by_field_name("name")?;
            (name.kind() == "identifier" && text(name, src).starts_with(char::is_uppercase))
                .then(|| call(name, src, None, CallForm::Ctor))
        }
        _ => None,
    }
}

fn c(n: Node, src: &[u8]) -> Option<Call> {
    if n.kind() != "call_expression" {
        return None;
    }
    let f = n.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => Some(call(f, src, None, CallForm::Bare)),
        "field_expression" => Some(receiver_call(
            f.child_by_field_name("argument"),
            f.child_by_field_name("field")?,
            src,
            &[],
        )),
        _ => None,
    }
}

fn swift(n: Node, src: &[u8]) -> Option<Call> {
    if n.kind() != "call_expression" {
        return None;
    }
    let callee = n.named_child(0)?;
    match callee.kind() {
        "simple_identifier" => Some(call(callee, src, None, CallForm::Bare)),
        "navigation_expression" => {
            let suffix = callee.child_by_field_name("suffix")?;
            let name = named_child_of_kind(suffix, "simple_identifier")?;
            Some(receiver_call(
                callee.child_by_field_name("target"),
                name,
                src,
                &["self"],
            ))
        }
        _ => None,
    }
}

fn shell(n: Node, src: &[u8]) -> Option<Call> {
    if n.kind() != "command" {
        return None;
    }
    Some(call(
        n.child_by_field_name("name")?,
        src,
        None,
        CallForm::Bare,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lists every named node kind this file matches on. A typo or grammar drift would make a rule
    /// silently never fire, so each kind must exist in its grammar.
    #[test]
    fn node_kinds_exist_in_grammar() {
        let ts_kinds: &[&str] = &[
            "call_expression",
            "member_expression",
            "new_expression",
            "identifier",
            "this",
        ];
        let jsx_kinds: &[&str] = &["jsx_opening_element", "jsx_self_closing_element"];
        let kinds: &[(Lang, &[&str])] = &[
            (
                Lang::Rust,
                &[
                    "call_expression",
                    "generic_function",
                    "scoped_identifier",
                    "scoped_type_identifier",
                    "generic_type",
                    "field_expression",
                    "field_identifier",
                    "identifier",
                    "self",
                    "macro_invocation",
                    "struct_expression",
                    "token_tree",
                ],
            ),
            (
                Lang::Python,
                &["call", "attribute", "decorator", "identifier"],
            ),
            (Lang::TypeScript, ts_kinds),
            (Lang::Tsx, &[ts_kinds, jsx_kinds].concat()),
            (Lang::JavaScript, &[ts_kinds, jsx_kinds].concat()),
            (
                Lang::C,
                &[
                    "call_expression",
                    "field_expression",
                    "field_identifier",
                    "identifier",
                ],
            ),
            (
                Lang::Swift,
                &[
                    "call_expression",
                    "navigation_expression",
                    "navigation_suffix",
                    "simple_identifier",
                ],
            ),
            (Lang::Shell, &["command", "command_name"]),
        ];
        for &(lang, names) in kinds {
            let Some(grammar) = lang.grammar() else {
                continue;
            };
            for name in names {
                assert_ne!(
                    grammar.id_for_node_kind(name, true),
                    0,
                    "{lang:?} has no named node kind {name:?}"
                );
            }
        }
    }

    #[test]
    fn calls_carry_the_byte_offset_of_their_name() {
        let src = "fn a() { b(); } fn b() {}\n";
        let (_, calls) = analyze(Lang::Rust, src).unwrap();
        let b = calls.iter().find(|c| c.name == "b").unwrap();
        assert_eq!((b.line, b.byte), (1, 9));
        assert_eq!(&src[b.byte..b.byte + 1], "b");
    }
}
