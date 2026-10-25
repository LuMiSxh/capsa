// SPDX-License-Identifier: MPL-2.0
//! Owns the symbol outline built from a tree-sitter parse: byte-to-line ranges, and the
//! attachment of doc comments and decorators, which move with the symbol they belong to.
//! Per-language node rules live in `rules/`.
use crate::dialect::{blank_gnu_c, blank_jsonc};
use crate::{Lang, rules};
use serde::Serialize;
use std::borrow::Cow;
use tree_sitter::{Node, Parser, Tree};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub kind: &'static str,
    /// Holds the name used in addresses (`impl Foo<T>` → `Foo`, heading text for Markdown).
    pub name: String,
    /// Holds an extra label for outlines, e.g. `Display for Foo<T>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Holds the `::`-joined names of the enclosing symbols and this one.
    pub qual: String,
    pub depth: usize,
    /// 1-based, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    #[serde(skip)]
    pub start_byte: usize,
    #[serde(skip)]
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outline {
    pub lang: Lang,
    pub lines: usize,
    pub symbols: Vec<Symbol>,
}

impl Outline {
    /// Returns the innermost symbol containing 1-based `line`.
    pub fn enclosing(&self, line: usize) -> Option<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.start_line <= line && line <= s.end_line)
            .max_by_key(|s| s.depth)
    }

    /// Returns the innermost symbol containing byte offset `byte`: calls on one line are told
    /// apart by it.
    pub fn enclosing_at(&self, byte: usize) -> Option<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.start_byte <= byte && byte < s.end_byte)
            .max_by_key(|s| s.depth)
    }

    /// Starts each symbol at its doc comments, attributes and decorators.
    pub fn with_attached(mut self, src: &str, path: &str) -> Self {
        let lines: Vec<_> = src.lines().collect();
        for s in &mut self.symbols {
            s.start_line = attached_start(&lines, path, s.start_line - 1) + 1;
        }
        self
    }
}

/// Returns the symbols of `src` in source order, or `None` when the grammar is not compiled in.
pub fn outline(lang: Lang, src: &str) -> Option<Outline> {
    parse_outline(lang, src).map(|(outline, _)| outline)
}

/// Returns the `outline`, also with the tree it was built from so `calls` can reuse the parse.
pub(crate) fn parse_outline(lang: Lang, src: &str) -> Option<(Outline, Tree)> {
    let mut symbols = Vec::new();
    let (rules_lang, tree) = match lang {
        Lang::Svelte => (Lang::TypeScript, svelte_scripts(src)?),
        _ => (lang, parse(lang, src, &[])?),
    };
    collect(
        rules_lang,
        tree.root_node(),
        src.as_bytes(),
        &mut Vec::new(),
        &mut symbols,
    );
    let outline = Outline {
        lang,
        lines: src.lines().count(),
        symbols,
    };
    Some((outline, tree))
}

/// Returns the 1-based lines of the error and missing nodes tree-sitter finds in `src`, in
/// document order, or `None` when the grammar is not compiled in. `jsonc` reads a JSON text as
/// JSONC; C text always has its GNU extensions blanked first.
pub fn syntax_errors(lang: Lang, src: &str, jsonc: bool) -> Option<Vec<usize>> {
    let text = match lang {
        Lang::C => blank_gnu_c(src),
        Lang::Json if jsonc => blank_jsonc(src),
        _ => Cow::Borrowed(src),
    };
    let tree = parse(lang, &text, &[])?;
    let mut lines = Vec::new();
    collect_errors(tree.root_node(), &mut lines);
    Some(lines)
}

fn collect_errors(node: tree_sitter::Node, lines: &mut Vec<usize>) {
    if !node.has_error() && !node.is_missing() {
        return;
    }
    if node.is_error() || node.is_missing() {
        lines.push(node.start_position().row + 1);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, lines);
    }
}

fn parse(lang: Lang, src: &str, ranges: &[tree_sitter::Range]) -> Option<Tree> {
    // One parser per thread: its internal buffers are reused across files.
    thread_local!(static PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new(Parser::new()));
    PARSER.with_borrow_mut(|p| {
        p.set_language(&lang.grammar()?).ok()?;
        // An empty slice resets to the whole document.
        p.set_included_ranges(ranges).ok()?;
        p.parse(src, None)
    })
}

/// Parses every `<script>` body of a Svelte file as TypeScript, keeping file positions.
fn svelte_scripts(src: &str) -> Option<Tree> {
    let doc = parse(Lang::Svelte, src, &[])?;
    let mut ranges = Vec::new();
    let mut cur = doc.root_node().walk();
    for el in doc
        .root_node()
        .named_children(&mut cur)
        .filter(|n| n.kind() == "script_element")
    {
        let mut c2 = el.walk();
        ranges.extend(
            el.named_children(&mut c2)
                .filter(|n| n.kind() == "raw_text")
                .map(|n| n.range()),
        );
    }
    if ranges.is_empty() {
        return parse(Lang::TypeScript, "", &[]);
    }
    parse(Lang::TypeScript, src, &ranges)
}

/// Returns the first line (0-based) of the doc comments, attributes and decorators directly above
/// the line `start`, so a symbol is shown and moved together with them.
pub fn attached_start<S: AsRef<str>>(lines: &[S], path: &str, mut start: usize) -> usize {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());
    if matches!(ext, Some("md" | "markdown" | "json")) {
        return start;
    }
    let hash = matches!(
        ext,
        Some("py" | "pyi" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh")
    );
    let markup = matches!(
        ext,
        Some("html" | "htm" | "xml" | "plist" | "xib" | "storyboard")
    );
    while start > 0 {
        let l = lines[start - 1].as_ref().trim_start();
        let attached = ["///", "//", "#[", "#![", "@", "/*", "*"]
            .iter()
            .any(|p| l.starts_with(p))
            || (hash && l.starts_with('#'))
            || (markup && l.starts_with("<!--"));
        if !attached {
            break;
        }
        start -= 1;
    }
    start
}

/// Extends a setext heading until the next heading or the end of its section.
fn setext_end(n: Node) -> Option<usize> {
    if n.kind() != "setext_heading" {
        return None;
    }
    let mut next = n.next_named_sibling();
    while let Some(s) = next {
        if matches!(s.kind(), "setext_heading" | "section") {
            return Some(s.start_position().row);
        }
        next = s.next_named_sibling();
    }
    Some(end_line(n.parent()?))
}

fn collect(lang: Lang, node: Node, src: &[u8], stack: &mut Vec<String>, out: &mut Vec<Symbol>) {
    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        let Some(c) = rules::classify(lang, child, src) else {
            collect(lang, child, src, stack, out);
            continue;
        };
        let qual = if stack.is_empty() {
            c.name.clone()
        } else {
            format!("{}::{}", stack.join("::"), c.name)
        };
        out.push(Symbol {
            kind: c.kind,
            name: c.name.clone(),
            detail: c.detail,
            qual,
            depth: stack.len(),
            start_line: child.start_position().row + 1,
            end_line: setext_end(child).unwrap_or_else(|| end_line(child)),
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
        });
        if !c.leaf {
            stack.push(c.name);
            collect(lang, child, src, stack, out);
            stack.pop();
        }
    }
}

/// Returns the last line with content: a node ending at column 0 really ends on the line before.
fn end_line(n: Node) -> usize {
    let e = n.end_position();
    if e.column == 0 && e.row > n.start_position().row {
        e.row
    } else {
        e.row + 1
    }
}
